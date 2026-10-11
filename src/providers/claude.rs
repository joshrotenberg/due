use std::{future::Future, io::ErrorKind as IoErrorKind};

use claude_wrapper::{Claude, ClaudeCommand, Error, OutputFormat, QueryCommand, QueryResult};
use serde_json::Value;

use super::Context;
use crate::{
    cli::{Effort, Provider, RunArgs},
    result::{ErrorKind, RunError, RunResult, Usage},
};

pub async fn execute<C>(run: &RunArgs, context: &Context, prompt: &str, cancel: C) -> RunResult
where
    C: Future<Output = ()> + Send,
{
    let mut command = QueryCommand::new(prompt)
        .prompt_via_stdin(true)
        .output_format(OutputFormat::Json);
    if let Some(model) = &run.model {
        command = command.model(model);
    }
    if let Some(effort) = run.effort {
        command = command.effort(match effort {
            Effort::Low => claude_wrapper::Effort::Low,
            Effort::Medium => claude_wrapper::Effort::Medium,
            Effort::High => claude_wrapper::Effort::High,
        });
    }
    if let Some(id) = &run.resume {
        command = command.resume(id);
    }
    if run.ephemeral {
        command = command.no_session_persistence();
    }
    command = command.extra_args(&run.native_args);
    if run.dry_run {
        return RunResult::planned(Provider::Claude, context.invocation(command.args(), prompt));
    }
    let mut builder = Claude::builder()
        .binary(&context.binary)
        .working_dir(&context.cwd);
    if let Some(timeout) = run.timeout {
        builder = builder.timeout(timeout);
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(error) => return failure(error),
    };
    match command.execute_cancellable(&client, cancel).await {
        Ok(output) => {
            let mut result = parse(&output.stdout);
            result.diagnostics = output.stderr;
            result
        }
        Err(error) => failure(error),
    }
}

fn parse(stdout: &str) -> RunResult {
    let result = RunResult::new(Provider::Claude);
    let document: Value = match serde_json::from_str(stdout) {
        Ok(value) => value,
        Err(error) => {
            return result.fail(RunError::protocol(format!("invalid Claude JSON: {error}")));
        }
    };
    // Verbose CLI settings return the transcript as an array; the terminal
    // result retains the same schema as ordinary JSON mode.
    let value = match &document {
        Value::Array(records) => match records
            .iter()
            .rev()
            .find(|record| record.get("type").and_then(Value::as_str) == Some("result"))
        {
            Some(value) => value,
            None => {
                return result.fail(RunError::protocol(
                    "Claude transcript has no terminal result",
                ));
            }
        },
        value => value,
    };
    if value.get("type").and_then(Value::as_str) != Some("result")
        || value.get("is_error").and_then(Value::as_bool).is_none()
        || value
            .get("subtype")
            .is_some_and(|subtype| !subtype.is_string())
    {
        return result.fail(RunError::protocol(
            "Claude did not return a terminal result object",
        ));
    }
    let query: QueryResult = match serde_json::from_value(value.clone()) {
        Ok(query) => query,
        Err(error) => {
            return result.fail(RunError::protocol(format!(
                "invalid Claude result: {error}"
            )));
        }
    };
    let mut result = RunResult {
        text: query.result,
        session_id: (!query.session_id.is_empty()).then_some(query.session_id),
        cost_usd: query.cost_usd,
        usage: query.usage.map(|usage| Usage {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_write_input_tokens: usage.cache_write_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
        }),
        ..result
    };
    if query.is_error
        || value
            .get("subtype")
            .and_then(Value::as_str)
            .is_some_and(|subtype| subtype != "success")
    {
        let message = value
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| {
                errors
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| {
                if result.text.is_empty() {
                    "Claude reported a failed turn".into()
                } else {
                    result.text.clone()
                }
            });
        return result.fail(RunError::new(ErrorKind::Provider, message));
    }
    if result.text.is_empty() {
        if let Some(structured) = value
            .get("structured_output")
            .filter(|value| !value.is_null())
        {
            result.text = structured.to_string();
        } else if value.get("result").and_then(Value::as_str).is_none() {
            return result.fail(RunError::protocol(
                "Claude's successful result is missing response text",
            ));
        }
    }
    result
}

fn failure(error: Error) -> RunResult {
    let mut result = RunResult::new(Provider::Claude);
    let failure = match error {
        Error::CommandFailed {
            exit_code,
            stdout,
            stderr,
            ..
        } => {
            let parsed = parse(&stdout);
            if !matches!(
                parsed.error.as_ref().map(|error| error.kind),
                Some(ErrorKind::Protocol)
            ) {
                result = parsed;
            }
            result.diagnostics = stderr;
            let message = result.error.take().map(|e| e.message).unwrap_or_else(|| {
                if result.diagnostics.trim().is_empty() {
                    format!("Claude exited with status {exit_code}")
                } else {
                    result.diagnostics.trim().to_owned()
                }
            });
            RunError::new(ErrorKind::Provider, message).with_exit_code(exit_code)
        }
        Error::Auth {
            message, exit_code, ..
        } => RunError::new(ErrorKind::Provider, message).with_exit_code(exit_code),
        Error::MaxTurnsExceeded {
            session_id,
            cost_usd,
            exit_code,
            ..
        } => {
            result.session_id = session_id;
            result.cost_usd = cost_usd;
            RunError::new(ErrorKind::Provider, "Claude reached its maximum turn count")
                .with_exit_code(exit_code)
        }
        Error::MaxBudgetExceeded {
            session_id,
            cost_usd,
            exit_code,
            ..
        } => {
            result.session_id = session_id;
            result.cost_usd = cost_usd;
            RunError::new(ErrorKind::Provider, "Claude reached its spending limit")
                .with_exit_code(exit_code)
        }
        Error::NotFound => RunError::new(ErrorKind::NotFound, "claude was not found on PATH"),
        Error::Timeout { timeout_seconds } => RunError::new(
            ErrorKind::Timeout,
            format!("Claude exceeded its execution timeout ({timeout_seconds}s)"),
        ),
        Error::Cancelled => RunError::new(ErrorKind::Cancelled, "Claude run cancelled"),
        Error::Io { source, .. } if source.kind() == IoErrorKind::NotFound => {
            RunError::new(ErrorKind::NotFound, source.to_string())
        }
        Error::Io { source, .. } => RunError::io("Claude process I/O", source),
        other => RunError::new(ErrorKind::Provider, other.to_string()),
    };
    result.fail(failure)
}
