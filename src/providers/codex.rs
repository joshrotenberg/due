use std::{future::Future, io::ErrorKind as IoErrorKind, path::Path, sync::Arc, time::Duration};

use codex_wrapper::{
    Codex, CodexCommand, Error, ExecCommand, ExecResumeCommand, JsonLineEvent, QueryResult,
    TerminationSignal, TokenUsage,
};

use super::Context;
use crate::{
    cli::{Provider, RunArgs},
    result::{ErrorKind, RunError, RunResult, Usage},
    telemetry::{ActivityKind, ActivityStatus, Progress},
};

enum Turn {
    Fresh(ExecCommand),
    Resume(ExecResumeCommand),
}

impl Turn {
    fn new(run: &RunArgs, prompt: &str, output_path: &str, schema_path: Option<&str>) -> Self {
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
            if let Some(path) = schema_path {
                command = command.output_schema(path);
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
            if let Some(path) = schema_path {
                command = command.output_schema(path);
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

    async fn stream<C, F>(&self, client: &Codex, cancel: C, handler: F) -> codex_wrapper::Result<()>
    where
        C: Future<Output = ()> + Send,
        F: FnMut(JsonLineEvent),
    {
        match self {
            Self::Fresh(cmd) => cmd.stream_cancellable(client, cancel, handler).await,
            Self::Resume(cmd) => cmd.stream_cancellable(client, cancel, handler).await,
        }
    }
}

pub async fn execute<C>(
    run: &RunArgs,
    context: &Context,
    prompt: &str,
    progress: &Progress,
    cancel: C,
) -> RunResult
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
    let command = Turn::new(
        run,
        prompt,
        path,
        context.schema.as_ref().map(|schema| schema.path()),
    );
    if run.dry_run {
        return RunResult::planned(Provider::Codex, context.invocation(command.args(), prompt));
    }
    let mut builder = Codex::builder()
        .binary(&context.binary)
        .working_dir(&context.cwd)
        // The CLI owns tool groups that a forced provider-group kill cannot
        // reach. Give its interrupt handler a bounded chance to clean them up.
        .termination_signal(TerminationSignal::Interrupt)
        .termination_grace(Duration::from_secs(2));
    if let Some(timeout) = run.timeout {
        builder = builder.timeout(timeout);
    }
    if progress.enabled() {
        let progress = progress.clone();
        builder = builder.on_spawn(Arc::new(move |info| progress.spawned(info.pid, info.pgid)));
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(error) => return failure(error, &output_path),
    };
    if progress.enabled() {
        let mut state = StreamState::default();
        let outcome = command
            .stream(&client, cancel, |event| state.observe(event, progress))
            .await;
        // The streaming API discards successful stderr. Failed processes still
        // carry diagnostics in their error; the default buffered path is unchanged.
        return match outcome {
            Ok(()) => state.finish(&output_path),
            Err(error) => state.fail(error, &output_path),
        };
    }
    match command.execute(&client, cancel).await {
        Ok(output) => {
            let mut result = parse(&output.stdout, &output_path);
            result.diagnostics = output.stderr;
            result
        }
        Err(error) => failure(error, &output_path),
    }
}

#[derive(Default)]
struct StreamState {
    thread_id: Option<String>,
    session_id: Option<String>,
    usage: Option<TokenUsage>,
    completed: bool,
    terminal_error: Option<RunError>,
    // Keep only the latest message for a partial failed response, never the
    // event history or tool contents. Successful text comes from the final file.
    last_assistant: String,
}

impl StreamState {
    fn observe(&mut self, event: JsonLineEvent, progress: &Progress) {
        if self.thread_id.is_none()
            && let Some(id) = event.thread_id().filter(|id| !id.is_empty())
        {
            self.thread_id = Some(id.to_owned());
            progress.session(id);
        }
        if self.session_id.is_none()
            && let Some(id) = event.session_id().filter(|id| !id.is_empty())
        {
            self.session_id = Some(id.to_owned());
            if self.thread_id.is_none() {
                progress.session(id);
            }
        }
        match event.event_type.as_str() {
            "turn.started" => progress.phase("turn_started"),
            "turn.completed" => {
                self.completed = true;
                self.usage = event.usage();
                progress.phase("turn_completed");
            }
            "turn.failed" => {
                self.terminal_error = Some(terminal_error(&event));
                progress.phase("turn_failed");
            }
            "error" => progress.phase("provider_error"),
            "item.started" | "item.updated" | "item.completed"
                if event.item_type() == Some("error") =>
            {
                // A native error item is a diagnostic, not an activity, and does
                // not decide the turn outcome. Only turn.failed does that.
                // Report completion only, just as assistant messages do. No
                // item history or raw diagnostic content needs to be retained.
                if event.event_type == "item.completed" {
                    progress.phase("provider_error");
                }
            }
            "item.started" | "item.updated" | "item.completed" => {
                emit_activity(&event, progress);
                if let Some(text) = event.agent_message_text() {
                    progress.assistant(text.len(), assistant_detail(&event));
                    self.last_assistant = text;
                }
            }
            _ => (),
        }
    }

    fn finish(self, output_path: &Path) -> RunResult {
        let mut result = RunResult {
            session_id: self.thread_id.or(self.session_id),
            usage: self.usage.map(normalize_usage),
            ..RunResult::new(Provider::Codex)
        };
        if let Some(error) = self.terminal_error {
            result.text = std::fs::read_to_string(output_path)
                .map(normalize_final_newline)
                .unwrap_or(self.last_assistant);
            return result.fail(error);
        }
        if !self.completed {
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

    fn fail(self, error: Error, output_path: &Path) -> RunResult {
        let terminal_failed = self.terminal_error.is_some();
        let mut result = self.finish(output_path);
        let mut process = match error {
            Error::Json { source, .. } => RunResult::new(Provider::Codex)
                .fail(RunError::protocol(format!("invalid Codex JSONL: {source}"))),
            other => failure(other, output_path),
        };
        result.diagnostics = process.diagnostics;
        let error = process
            .error
            .take()
            .expect("process failure carries an error");
        if terminal_failed && error.kind == ErrorKind::Provider {
            // Streaming errors have no stdout. Preserve the terminal event's
            // explanation and partial result while adding the process exit code.
            result.error.as_mut().unwrap().provider_exit_code = error.provider_exit_code;
            result
        } else {
            // Cancellation, timeout, and transport errors override an earlier
            // native terminal event only after the wrapper has settled cleanup.
            result.fail(error)
        }
    }
}

// Detail must use explicit user-facing text fields, independently of the
// wrapper's permissive final-message compatibility fallback.
fn assistant_detail(event: &JsonLineEvent) -> Option<&str> {
    if event.item_type() != Some("agent_message") {
        return None;
    }
    let item = event.extra.get("item")?;
    item.get("text")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            item.get("content")?.as_array()?.iter().find_map(|block| {
                (block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(serde_json::Value::as_str))
                    .flatten()
            })
        })
}

fn activity_kind(item_type: &str) -> Option<ActivityKind> {
    Some(match item_type {
        // These are assistant content, not tools or observable actions.
        "agent_message" | "reasoning" => return None,
        // Native diagnostics are reported as provider_error phases instead.
        "error" => return None,
        "command_execution" => ActivityKind::Command,
        "file_change" => ActivityKind::FileChange,
        "mcp_tool_call" => ActivityKind::McpCall,
        "web_search" => ActivityKind::WebSearch,
        "todo_list" | "plan" => ActivityKind::PlanUpdate,
        "status_update" => ActivityKind::StatusUpdate,
        _ => ActivityKind::Unknown,
    })
}

fn activity_status(item: &serde_json::Value) -> ActivityStatus {
    match (
        item.get("status").and_then(serde_json::Value::as_str),
        item.get("exit_code").and_then(serde_json::Value::as_i64),
    ) {
        (Some("cancelled"), _) => ActivityStatus::Cancelled,
        (_, Some(code)) if code != 0 => ActivityStatus::Failed,
        (Some("failed"), _) => ActivityStatus::Failed,
        (Some("completed" | "succeeded"), _) | (_, Some(0)) => ActivityStatus::Succeeded,
        _ => ActivityStatus::Unknown,
    }
}

fn emit_activity(event: &JsonLineEvent, progress: &Progress) {
    if !matches!(event.event_type.as_str(), "item.started" | "item.completed") {
        return;
    }
    let Some(item_type) = event.item_type() else {
        return;
    };
    let Some(kind) = activity_kind(item_type) else {
        return;
    };
    let Some(item) = event.extra.get("item") else {
        return;
    };
    let id = item.get("id").and_then(serde_json::Value::as_str);
    // Keep tool names static: native command text, MCP identifiers, search
    // queries, paths, and unfamiliar item type strings never enter the trace.
    let tool = match item_type {
        "command_execution" => Some("command_execution"),
        "mcp_tool_call" => Some("mcp_tool_call"),
        _ => None,
    };
    let detail = if progress.detail_enabled() {
        match item_type {
            "command_execution" => item.get("command").and_then(serde_json::Value::as_str),
            "file_change" => item
                .pointer("/changes/0/path")
                .and_then(serde_json::Value::as_str),
            "mcp_tool_call" => item.get("tool").and_then(serde_json::Value::as_str),
            _ => None,
        }
    } else {
        None
    };
    if event.event_type == "item.started" {
        progress.activity_started(id, kind, tool, detail);
    } else {
        progress.activity_completed(id, kind, activity_status(item), tool, detail);
    }
}

fn normalize_usage(usage: TokenUsage) -> Usage {
    Usage {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        total_tokens: usage.total_tokens,
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
        return result.fail(terminal_error(failed));
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

fn terminal_error(event: &JsonLineEvent) -> RunError {
    // This machine-readable code is captured by the pinned wrapper. Unknown
    // API codes and prose (including rollout-budget text) stay provider errors.
    let kind = match event
        .turn_failure_api_error()
        .and_then(|failure| failure.code)
        .as_deref()
    {
        Some("invalid_json_schema") => ErrorKind::Configuration,
        _ => ErrorKind::Provider,
    };
    RunError::new(
        kind,
        event
            .turn_failure_message()
            .unwrap_or("Codex reported a failed turn"),
    )
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
            if matches!(
                parsed.error.as_ref().map(|error| error.kind),
                Some(ErrorKind::Protocol)
            ) {
                // A failed process can stop before its terminal event or final
                // file. Preserve observed metadata while reporting the actual
                // process failure, just as the streaming path does.
                result.session_id = parsed.session_id;
                result.usage = parsed.usage;
            } else {
                result = parsed;
            }
            result.diagnostics = stderr;
            result
                .error
                .take()
                .unwrap_or_else(|| {
                    let message = if result.diagnostics.trim().is_empty() {
                        format!("Codex exited with status {exit_code}")
                    } else {
                        result.diagnostics.trim().to_owned()
                    };
                    RunError::new(ErrorKind::Provider, message)
                })
                .with_exit_code(exit_code)
        }
        Error::Auth {
            message, exit_code, ..
        } => RunError::new(ErrorKind::Authentication, message).with_exit_code(exit_code),
        Error::Config {
            message, exit_code, ..
        }
        | Error::NotTrustedDirectory {
            message, exit_code, ..
        } => RunError::new(ErrorKind::Configuration, message).with_exit_code(exit_code),
        Error::SessionNotFound {
            message, exit_code, ..
        } => RunError::new(ErrorKind::SessionNotFound, message).with_exit_code(exit_code),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn schema_path_is_forwarded_for_fresh_and_resumed_turns() {
        for resume in [false, true] {
            let mut args = vec!["due", "run", "--provider", "codex", "--prompt", "prompt"];
            if resume {
                args.extend(["--resume", "thread-1"]);
            }
            let crate::cli::Cli {
                command: crate::cli::Command::Run(run),
            } = crate::cli::Cli::try_parse_from(args).unwrap()
            else {
                panic!("expected run command");
            };
            let run = crate::request::resolve(*run).unwrap();
            let command = Turn::new(&run, "prompt", "/tmp/final-message", Some("/tmp/schema"));
            let argv = command.args();
            let schema = argv
                .iter()
                .position(|arg| arg == "--output-schema")
                .unwrap();
            assert_eq!(argv[schema + 1], "/tmp/schema");
            assert_eq!(argv.last().unwrap(), "-");
            if resume {
                let session = argv.iter().position(|arg| arg == "thread-1").unwrap();
                assert!(schema < session);
            }
            assert!(
                !Turn::new(&run, "prompt", "/tmp/final-message", None)
                    .args()
                    .iter()
                    .any(|arg| arg == "--output-schema")
            );
        }
    }

    #[test]
    fn native_activity_kinds_do_not_treat_assistant_content_as_activity() {
        for (native, expected) in [
            ("command_execution", ActivityKind::Command),
            ("file_change", ActivityKind::FileChange),
            ("mcp_tool_call", ActivityKind::McpCall),
            ("web_search", ActivityKind::WebSearch),
            ("todo_list", ActivityKind::PlanUpdate),
            ("plan", ActivityKind::PlanUpdate),
            ("status_update", ActivityKind::StatusUpdate),
            ("future_native_item", ActivityKind::Unknown),
        ] {
            assert_eq!(activity_kind(native), Some(expected));
        }
        assert_eq!(activity_kind("agent_message"), None);
        assert_eq!(activity_kind("reasoning"), None);
        assert_eq!(activity_kind("error"), None);
    }

    #[test]
    fn native_error_items_are_diagnostics_that_keep_turn_outcomes() {
        use serde_json::json;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("final-message.txt");
        let diagnostic = |kind: &str, id: &str| {
            serde_json::from_value::<JsonLineEvent>(json!({
                "type":kind,
                "item":{"id":id,"type":"error","message":"SECRET_DIAGNOSTIC_MESSAGE"}
            }))
            .unwrap()
        };
        for failed in [false, true] {
            let mut state = StreamState::default();
            let mut events = vec![diagnostic("item.completed", "item_0")];
            for kind in ["item.started", "item.updated", "item.completed"] {
                events.push(diagnostic(kind, "item_1"));
            }
            events.push(serde_json::from_value(json!({"type":"turn.started"})).unwrap());
            events.push(
                serde_json::from_value(if failed {
                    json!({"type":"turn.failed","error":{"message":"native terminal failure"}})
                } else {
                    json!({"type":"turn.completed"})
                })
                .unwrap(),
            );
            for event in events {
                state.observe(event, &Progress::default());
            }
            assert!(state.last_assistant.is_empty());
            std::fs::write(&path, "final answer\n").unwrap();
            let result = state.finish(&path);
            if failed {
                let error = result.error.unwrap();
                assert_eq!(error.kind, ErrorKind::Provider);
                assert_eq!(error.message, "native terminal failure");
            } else {
                assert!(result.error.is_none());
                assert_eq!(result.text, "final answer");
            }
        }
    }

    #[test]
    fn activity_completion_requires_native_outcome_evidence() {
        use serde_json::json;
        for (item, expected) in [
            (json!({}), ActivityStatus::Unknown),
            (json!({"status":"in_progress"}), ActivityStatus::Unknown),
            (json!({"status":"future_status"}), ActivityStatus::Unknown),
            (json!({"exit_code":"0"}), ActivityStatus::Unknown),
            (json!({"exit_code":0}), ActivityStatus::Succeeded),
            (json!({"status":"completed"}), ActivityStatus::Succeeded),
            (json!({"status":"succeeded"}), ActivityStatus::Succeeded),
            (
                json!({"status":"failed","exit_code":0}),
                ActivityStatus::Failed,
            ),
            (
                json!({"status":"completed","exit_code":1}),
                ActivityStatus::Failed,
            ),
            (
                json!({"status":"cancelled","exit_code":1}),
                ActivityStatus::Cancelled,
            ),
        ] {
            assert_eq!(activity_status(&item), expected);
        }
    }

    #[test]
    fn typed_wrapper_errors_preserve_common_categories_and_exit_codes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-written.txt");
        for (error, expected) in [
            (
                Error::Auth {
                    message: "login required".into(),
                    command: "unused".into(),
                    exit_code: 7,
                    working_dir: None,
                },
                ErrorKind::Authentication,
            ),
            (
                Error::Config {
                    message: "invalid config".into(),
                    command: "unused".into(),
                    exit_code: 7,
                    working_dir: None,
                },
                ErrorKind::Configuration,
            ),
            (
                Error::NotTrustedDirectory {
                    message: "directory not trusted".into(),
                    command: "unused".into(),
                    exit_code: 7,
                    working_dir: None,
                },
                ErrorKind::Configuration,
            ),
            (
                Error::SessionNotFound {
                    message: "session does not exist".into(),
                    command: "unused".into(),
                    exit_code: 7,
                    working_dir: None,
                },
                ErrorKind::SessionNotFound,
            ),
        ] {
            let error = failure(error, &path).error.unwrap();
            assert_eq!(error.kind, expected);
            assert_eq!(error.provider_exit_code, Some(7));
            assert_eq!(error.exit_code(), 1);
        }
    }

    #[test]
    fn only_verified_terminal_codes_are_classified_across_execution_modes() {
        use serde_json::json;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-written.txt");
        for (code, expected) in [
            ("invalid_json_schema", ErrorKind::Configuration),
            ("future_schema_error", ErrorKind::Provider),
        ] {
            let message = json!({
                "type":"error",
                "error":{"type":"invalid_request_error","code":code},
                "status":400
            })
            .to_string();
            let event: JsonLineEvent = serde_json::from_value(json!({
                "type":"turn.failed", "error":{"message":message}
            }))
            .unwrap();
            assert_eq!(terminal_error(&event).kind, expected);
            let stdout = serde_json::to_string(&event).unwrap();
            let buffered = failure(
                Error::CommandFailed {
                    command: "unused".into(),
                    exit_code: 9,
                    stdout,
                    stderr: String::new(),
                    working_dir: None,
                },
                &path,
            );
            let mut state = StreamState::default();
            state.observe(event, &Progress::default());
            let streamed = state.fail(
                Error::CommandFailed {
                    command: "unused".into(),
                    exit_code: 9,
                    stdout: String::new(),
                    stderr: String::new(),
                    working_dir: None,
                },
                &path,
            );
            for result in [buffered, streamed] {
                let error = result.error.unwrap();
                assert_eq!(error.kind, expected);
                assert_eq!(error.message, message);
                assert_eq!(error.provider_exit_code, Some(9));
            }
        }
        let plain: JsonLineEvent = serde_json::from_value(json!({
            "type":"turn.failed", "error":{"message":"invalid_json_schema"}
        }))
        .unwrap();
        assert_eq!(terminal_error(&plain).kind, ErrorKind::Provider);
    }

    #[test]
    fn process_failure_preserves_observed_metadata_without_a_complete_response() {
        use serde_json::json;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-written.txt");
        for completed in [false, true] {
            let mut events = vec![json!({"type":"thread.started","thread_id":"thread-partial"})];
            if completed {
                events.push(json!({
                    "type":"turn.completed",
                    "usage":{"input_tokens":11,"output_tokens":3}
                }));
            }
            let stdout = events
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            let buffered = failure(
                Error::CommandFailed {
                    command: "unused".into(),
                    exit_code: 7,
                    stdout,
                    stderr: "process interrupted".into(),
                    working_dir: None,
                },
                &path,
            );
            let mut state = StreamState::default();
            for event in events {
                state.observe(serde_json::from_value(event).unwrap(), &Progress::default());
            }
            let streamed = state.fail(
                Error::CommandFailed {
                    command: "unused".into(),
                    exit_code: 7,
                    stdout: String::new(),
                    stderr: "process interrupted".into(),
                    working_dir: None,
                },
                &path,
            );
            for result in [buffered, streamed] {
                assert_eq!(result.session_id.as_deref(), Some("thread-partial"));
                assert_eq!(result.usage.is_some(), completed);
                if let Some(usage) = result.usage {
                    assert_eq!(usage.input_tokens, Some(11));
                    assert_eq!(usage.output_tokens, Some(3));
                }
                let error = result.error.unwrap();
                assert_eq!(error.kind, ErrorKind::Provider);
                assert_eq!(error.provider_exit_code, Some(7));
                assert_eq!(error.exit_code(), 1);
            }
        }
    }

    #[test]
    fn streamed_terminal_failure_survives_a_process_error_without_stdout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-written.txt");
        let state = StreamState {
            thread_id: Some("thread-1".into()),
            terminal_error: Some(RunError::new(
                ErrorKind::Provider,
                "native terminal failure",
            )),
            last_assistant: "partial answer".into(),
            ..StreamState::default()
        };
        let result = state.fail(
            Error::CommandFailed {
                command: "unused command".into(),
                exit_code: 9,
                stdout: String::new(),
                stderr: "native diagnostic".into(),
                working_dir: None,
            },
            &path,
        );
        assert_eq!(result.session_id.as_deref(), Some("thread-1"));
        assert_eq!(result.text, "partial answer");
        assert_eq!(result.diagnostics, "native diagnostic");
        let error = result.error.unwrap();
        assert_eq!(error.kind, ErrorKind::Provider);
        assert_eq!(error.message, "native terminal failure");
        assert_eq!(error.provider_exit_code, Some(9));
    }

    #[test]
    fn streamed_cancellation_and_timeout_override_native_terminal_failure() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-written.txt");
        for (error, expected) in [
            (Error::Cancelled { grace_seconds: 5 }, ErrorKind::Cancelled),
            (Error::Timeout { timeout_seconds: 5 }, ErrorKind::Timeout),
        ] {
            let state = StreamState {
                terminal_error: Some(RunError::new(ErrorKind::Provider, "earlier native failure")),
                ..StreamState::default()
            };
            assert_eq!(state.fail(error, &path).error.unwrap().kind, expected);
        }
    }

    #[test]
    fn streamed_success_requires_terminal_and_reads_final_file_not_commentary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("final-message.txt");
        std::fs::write(&path, "final answer\n").unwrap();
        let incomplete = StreamState::default().finish(&path);
        assert_eq!(incomplete.error.unwrap().kind, ErrorKind::Protocol);
        let state = StreamState {
            completed: true,
            last_assistant: "earlier commentary".into(),
            ..StreamState::default()
        };
        let result = state.finish(&path);
        assert!(result.error.is_none());
        assert_eq!(result.text, "final answer");
        std::fs::remove_file(&path).unwrap();
        let missing_file = StreamState {
            completed: true,
            ..StreamState::default()
        }
        .finish(&path);
        assert_eq!(missing_file.error.unwrap().kind, ErrorKind::Protocol);
    }
}
