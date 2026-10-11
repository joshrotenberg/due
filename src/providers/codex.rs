use std::{future::Future, io::ErrorKind as IoErrorKind, path::Path};

use codex_wrapper::{
    Codex, CodexCommand, Error, ExecCommand, ExecResumeCommand, JsonLineEvent, QueryResult,
};

use super::Context;
use crate::{
    cli::{Provider, RunArgs},
    result::{ErrorKind, RunError, RunResult, Usage},
};

enum Turn {
    Fresh(ExecCommand),
    Resume(ExecResumeCommand),
}

impl Turn {
    fn new(run: &RunArgs, prompt: &str, output_path: &str) -> Self {
        // The wrapper has separate typed builders because native exec/resume
        // accept different option sets. Keep that distinction at this boundary.
        if let Some(id) = &run.resume {
            let mut command = ExecResumeCommand::from_stdin(prompt)
                .session_id(id)
                .json()
                .output_last_message(output_path);
            if let Some(model) = &run.model {
                command = command.model(model);
            }
            if let Some(effort) = run.effort {
                command = command.config(format!("model_reasoning_effort=\"{}\"", effort.as_str()));
            }
            Self::Resume(command.extra_args(&run.native_args))
        } else {
            let mut command = ExecCommand::from_stdin(prompt)
                .json()
                .output_last_message(output_path);
            if let Some(model) = &run.model {
                command = command.model(model);
            }
            if let Some(effort) = run.effort {
                command = command.config(format!("model_reasoning_effort=\"{}\"", effort.as_str()));
            }
            if run.ephemeral {
                command = command.ephemeral();
            }
            Self::Fresh(command.extra_args(&run.native_args))
        }
    }

    fn args(&self) -> Vec<String> {
        match self {
            Self::Fresh(cmd) => cmd.args(),
            Self::Resume(cmd) => cmd.args(),
        }
    }

    async fn execute<C>(
        &self,
        client: &Codex,
        cancel: C,
    ) -> codex_wrapper::Result<codex_wrapper::exec::CommandOutput>
    where
        C: Future<Output = ()> + Send,
    {
        match self {
            Self::Fresh(cmd) => cmd.execute_cancellable(client, cancel).await,
            Self::Resume(cmd) => cmd.execute_cancellable(client, cancel).await,
        }
    }
}

pub async fn execute<C>(run: &RunArgs, context: &Context, prompt: &str, cancel: C) -> RunResult
where
    C: Future<Output = ()> + Send,
{
    // A private per-run directory keeps concurrent invocations independent and
    // distinguishes an absent final message from an intentionally empty one.
    let temporary = match tempfile::Builder::new().prefix("due-").tempdir() {
        Ok(dir) => dir,
        Err(error) => {
            return RunResult::new(Provider::Codex)
                .fail(RunError::io("create final-message directory", error));
        }
    };
    let output_path = temporary.path().join("final-message.txt");
    let Some(path) = output_path.to_str() else {
        return RunResult::new(Provider::Codex).fail(RunError::invalid(
            "Codex requires a UTF-8 temporary directory path",
        ));
    };
    let command = Turn::new(run, prompt, path);
    if run.dry_run {
        return RunResult::planned(Provider::Codex, context.invocation(command.args(), prompt));
    }
    let mut builder = Codex::builder()
        .binary(&context.binary)
        .working_dir(&context.cwd);
    if let Some(timeout) = run.timeout {
        builder = builder.timeout(timeout);
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(error) => return failure(error, &output_path),
    };
    match command.execute(&client, cancel).await {
        Ok(output) => {
            let mut result = parse(&output.stdout, &output_path);
            result.diagnostics = output.stderr;
            result
        }
        Err(error) => failure(error, &output_path),
    }
}

fn parse(stdout: &str, output_path: &Path) -> RunResult {
    let mut events = Vec::new();
    for (index, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<JsonLineEvent>(line) {
            Ok(event) => events.push(event),
            Err(error) => {
                return RunResult::new(Provider::Codex).fail(RunError::protocol(format!(
                    "invalid Codex JSONL on line {}: {error}",
                    index + 1
                )));
            }
        }
    }
    let query = QueryResult::from_events(events);
    let mut result = RunResult {
        session_id: query
            .thread_id
            .or(query.session_id)
            .filter(|id| !id.is_empty()),
        usage: query.usage.map(|usage| Usage {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_write_input_tokens: usage.cache_write_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
        }),
        ..RunResult::new(Provider::Codex)
    };
    if let Some(failed) = query
        .events
        .iter()
        .rev()
        .find(|event| event.is_turn_failed())
    {
        // Preserve useful partial text in JSON, but never print it as a successful
        // text-mode answer. Native failures may not write the final-message file.
        result.text = std::fs::read_to_string(output_path)
            .map(normalize_final_newline)
            .unwrap_or_else(|_| {
                query
                    .events
                    .iter()
                    .rev()
                    .find_map(JsonLineEvent::agent_message_text)
                    .unwrap_or_default()
            });
        return result.fail(RunError::new(
            ErrorKind::Provider,
            failed
                .turn_failure_message()
                .unwrap_or("Codex reported a failed turn"),
        ));
    }
    if !query.events.iter().any(JsonLineEvent::is_turn_completed) {
        return result.fail(RunError::protocol("Codex did not emit turn.completed"));
    }
    match std::fs::read_to_string(output_path) {
        Ok(text) => result.text = normalize_final_newline(text),
        Err(error) => {
            return result.fail(RunError::protocol(format!(
                "Codex did not produce a readable final-message file: {error}"
            )));
        }
    }
    result
}

fn normalize_final_newline(mut text: String) -> String {
    // The CLI's output file adds a line terminator. Leave other whitespace alone.
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}

fn failure(error: Error, output_path: &Path) -> RunResult {
    let mut result = RunResult::new(Provider::Codex);
    let failure = match error {
        Error::CommandFailed {
            exit_code,
            stdout,
            stderr,
            ..
        } => {
            let parsed = parse(&stdout, output_path);
            if !matches!(
                parsed.error.as_ref().map(|error| error.kind),
                Some(ErrorKind::Protocol)
            ) {
                result = parsed;
            }
            result.diagnostics = stderr;
            let message = result.error.take().map(|e| e.message).unwrap_or_else(|| {
                if result.diagnostics.trim().is_empty() {
                    format!("Codex exited with status {exit_code}")
                } else {
                    result.diagnostics.trim().to_owned()
                }
            });
            RunError::new(ErrorKind::Provider, message).with_exit_code(exit_code)
        }
        Error::Auth {
            message, exit_code, ..
        }
        | Error::Config {
            message, exit_code, ..
        }
        | Error::NotTrustedDirectory {
            message, exit_code, ..
        }
        | Error::SessionNotFound {
            message, exit_code, ..
        } => RunError::new(ErrorKind::Provider, message).with_exit_code(exit_code),
        Error::NotFound => RunError::new(ErrorKind::NotFound, "codex was not found on PATH"),
        Error::Timeout { timeout_seconds } => RunError::new(
            ErrorKind::Timeout,
            format!("Codex exceeded its execution timeout ({timeout_seconds}s)"),
        ),
        Error::Cancelled { .. } => RunError::new(ErrorKind::Cancelled, "Codex run cancelled"),
        Error::Io { source, .. } if source.kind() == IoErrorKind::NotFound => {
            RunError::new(ErrorKind::NotFound, source.to_string())
        }
        Error::Io { source, .. } => RunError::io("Codex process I/O", source),
        other => RunError::new(ErrorKind::Provider, other.to_string()),
    };
    result.fail(failure)
}
