use std::{collections::HashMap, future::Future, io::ErrorKind as IoErrorKind, sync::Arc};

use claude_wrapper::streaming::{
    BlockType, PartialMessageEvent, StreamEvent, stream_query_cancellable,
};
use claude_wrapper::{Claude, ClaudeCommand, Error, OutputFormat, QueryCommand, QueryResult};
use serde_json::Value;

use super::Context;
use crate::{
    cli::{Effort, Provider, RunArgs},
    result::{ErrorKind, RunError, RunResult, Usage},
    telemetry::Progress,
};

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
    if progress.enabled() {
        command = command
            .output_format(OutputFormat::StreamJson)
            .include_partial_messages();
    }
    if run.dry_run {
        return RunResult::planned(Provider::Claude, context.invocation(command.args(), prompt));
    }
    let mut builder = Claude::builder()
        .binary(&context.binary)
        .working_dir(&context.cwd);
    if let Some(timeout) = run.timeout {
        builder = builder.timeout(timeout);
    }
    if progress.enabled() {
        let observer = progress.clone();
        builder = builder.on_spawn(Arc::new(move |info| {
            observer.spawned(info.pid, info.pgid);
        }));
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(error) => return failure(error),
    };
    if progress.enabled() {
        let mut state = StreamState::default();
        let outcome = stream_query_cancellable(&client, &command, cancel, |event| {
            state.observe(event, progress);
        })
        .await;
        return state.finish(outcome);
    }
    match command.execute_cancellable(&client, cancel).await {
        Ok(output) => {
            let mut result = parse(&output.stdout);
            result.diagnostics = output.stderr;
            result
        }
        Err(error) => failure(error),
    }
}

// Keep only the final response and bounded tool-correlation metadata, never a
// transcript. A content-block stop is not evidence that its tool has finished.
#[derive(Default)]
struct StreamState {
    terminal: Option<Value>,
    session_id: Option<String>,
    active_tools: HashMap<String, String>,
    turn_started: bool,
}

impl StreamState {
    fn observe(&mut self, event: StreamEvent, progress: &Progress) {
        if let Some(id) = event.session_id().filter(|id| !id.is_empty()) {
            progress.session(id);
            if self.session_id.is_none() {
                self.session_id = Some(id.to_owned());
            }
        }
        match event.event_type() {
            Some("system") if event.data.get("subtype").and_then(Value::as_str) == Some("init") => {
                if !self.turn_started {
                    self.turn_started = true;
                    progress.phase("turn_started");
                }
            }
            Some("stream_event") => {
                // Inspect starts only: parsing/logging deltas would expose text,
                // reasoning, or tool arguments and produce per-token noise.
                if event.data.pointer("/event/type").and_then(Value::as_str)
                    == Some("content_block_start")
                    && let Some(PartialMessageEvent::BlockStart { block_type, .. }) =
                        event.partial_message()
                {
                    match block_type {
                        BlockType::ToolUse { id, name } => {
                            self.tool_started(Some(&id), Some(&name), progress);
                        }
                        BlockType::Thinking => progress.phase("thinking_started"),
                        BlockType::Text => progress.phase("response_started"),
                        BlockType::Other(_) => (),
                    }
                }
            }
            Some("assistant") => {
                let content = event.data.pointer("/message/content");
                let mut bytes = content.and_then(Value::as_str).map_or(0, str::len);
                if let Some(blocks) = content.and_then(Value::as_array) {
                    for block in blocks {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                bytes = bytes.saturating_add(
                                    block
                                        .get("text")
                                        .and_then(Value::as_str)
                                        .map_or(0, str::len),
                                );
                            }
                            Some("tool_use") => self.tool_started(
                                block.get("id").and_then(Value::as_str),
                                block.get("name").and_then(Value::as_str),
                                progress,
                            ),
                            _ => (),
                        }
                    }
                }
                progress.assistant(bytes);
            }
            Some("user") => {
                if let Some(blocks) = event
                    .data
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                            continue;
                        }
                        let id = block.get("tool_use_id").and_then(Value::as_str);
                        let name = id.and_then(|id| self.active_tools.remove(id));
                        let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                        progress.item(
                            if failed {
                                "tool_failed"
                            } else {
                                "tool_completed"
                            },
                            id,
                            "tool",
                            name.as_deref().filter(|name| !name.is_empty()),
                        );
                    }
                }
            }
            Some("result") => {
                // Native semantic failure is independent of the process exit
                // status; run_finished remains authoritative after cleanup.
                let subtype = event.data.get("subtype");
                if let Some(failed) = event.data.get("is_error").and_then(Value::as_bool)
                    && subtype.is_none_or(Value::is_string)
                {
                    progress.phase(
                        if failed
                            || subtype
                                .and_then(Value::as_str)
                                .is_some_and(|subtype| subtype != "success")
                        {
                            "turn_failed"
                        } else {
                            "turn_completed"
                        },
                    );
                }
                self.terminal = Some(event.data);
            }
            _ => (),
        }
    }

    fn tool_started(&mut self, id: Option<&str>, name: Option<&str>, progress: &Progress) {
        const MAX_ACTIVE_TOOLS: usize = 1024;
        let id = id.filter(|id| !id.is_empty());
        if let Some(id) = id {
            if self.active_tools.contains_key(id) || self.active_tools.len() >= MAX_ACTIVE_TOOLS {
                return;
            }
            self.active_tools
                .insert(id.to_owned(), name.unwrap_or_default().to_owned());
        }
        progress.item(
            "tool_started",
            id,
            "tool",
            name.filter(|name| !name.is_empty()),
        );
    }

    fn finish(self, outcome: claude_wrapper::Result<claude_wrapper::CommandOutput>) -> RunResult {
        let terminal = self.terminal.as_ref().map(parse_document);
        let mut result = match outcome {
            Ok(output) => {
                let mut result = terminal.unwrap_or_else(|| {
                    RunResult::new(Provider::Claude)
                        .fail(RunError::protocol("Claude stream has no terminal result"))
                });
                result.diagnostics = output.stderr;
                result
            }
            Err(error) => {
                let mut failed = failure(error);
                match terminal {
                    Some(mut result)
                        if !matches!(
                            result.error.as_ref().map(|error| error.kind),
                            Some(ErrorKind::Protocol)
                        ) =>
                    {
                        // Stream callbacks own valid JSON events; the wrapper's
                        // error stdout contains only non-JSON diagnostics. Merge
                        // the retained terminal result with the process failure.
                        let mut error = failed.error.take().expect("failure carries an error");
                        if error.kind == ErrorKind::Provider
                            && let Some(terminal_error) = result.error.take()
                        {
                            error.message = terminal_error.message;
                        }
                        result.diagnostics = failed.diagnostics;
                        result.fail(error)
                    }
                    _ => failed,
                }
            }
        };
        if result.session_id.is_none() {
            result.session_id = self.session_id;
        }
        result
    }
}

fn parse(stdout: &str) -> RunResult {
    let document: Value = match serde_json::from_str(stdout) {
        Ok(value) => value,
        Err(error) => {
            return RunResult::new(Provider::Claude)
                .fail(RunError::protocol(format!("invalid Claude JSON: {error}")));
        }
    };
    parse_document(&document)
}

fn parse_document(document: &Value) -> RunResult {
    let result = RunResult::new(Provider::Claude);
    // Verbose CLI settings return the transcript as an array; the terminal
    // result retains the same schema as ordinary JSON mode.
    let value = match document {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn output() -> claude_wrapper::CommandOutput {
        claude_wrapper::CommandOutput {
            stdout: String::new(),
            stderr: "native diagnostic".into(),
            exit_code: 0,
            success: true,
        }
    }

    fn terminal() -> Value {
        json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true,
            "result": "partial answer", "errors": ["native terminal failure"],
            "session_id": "session-1", "total_cost_usd": 0.03,
            "usage": {"input_tokens": 120, "output_tokens": 45}
        })
    }

    #[test]
    fn streamed_terminal_failure_survives_a_process_error_without_stdout() {
        let state = StreamState {
            terminal: Some(terminal()),
            ..StreamState::default()
        };
        let result = state.finish(Err(Error::CommandFailed {
            command: "unused command".into(),
            exit_code: 9,
            stdout: String::new(),
            stderr: "native diagnostic".into(),
            working_dir: None,
        }));
        assert_eq!(result.session_id.as_deref(), Some("session-1"));
        assert_eq!(result.text, "partial answer");
        assert_eq!(result.cost_usd, Some(0.03));
        assert_eq!(result.usage.unwrap().output_tokens, Some(45));
        assert_eq!(result.diagnostics, "native diagnostic");
        let error = result.error.unwrap();
        assert_eq!(error.kind, ErrorKind::Provider);
        assert_eq!(error.message, "native terminal failure");
        assert_eq!(error.provider_exit_code, Some(9));
    }

    #[test]
    fn streamed_cancellation_and_timeout_override_native_terminal_failure() {
        for (error, expected) in [
            (Error::Cancelled, ErrorKind::Cancelled),
            (Error::Timeout { timeout_seconds: 5 }, ErrorKind::Timeout),
        ] {
            let state = StreamState {
                terminal: Some(terminal()),
                ..StreamState::default()
            };
            let result = state.finish(Err(error));
            assert_eq!(result.error.unwrap().kind, expected);
            assert_eq!(result.session_id.as_deref(), Some("session-1"));
            assert_eq!(result.text, "partial answer");
        }
    }

    #[test]
    fn streamed_last_terminal_matches_buffered_structured_output() {
        let mut state = StreamState::default();
        let final_result = json!({
            "type": "result", "subtype": "success", "is_error": false,
            "result": "", "structured_output": {"answer": 42}, "session_id": "session-1"
        });
        for data in [terminal(), final_result.clone()] {
            state.observe(StreamEvent { data }, &Progress::default());
        }
        let streamed = state.finish(Ok(output()));
        assert_eq!(streamed.diagnostics, "native diagnostic");
        let buffered = parse(&final_result.to_string());
        assert_eq!(
            serde_json::to_value(streamed).unwrap(),
            serde_json::to_value(buffered).unwrap()
        );
        let missing = StreamState::default().finish(Ok(output()));
        assert_eq!(missing.error.unwrap().kind, ErrorKind::Protocol);
    }
}
