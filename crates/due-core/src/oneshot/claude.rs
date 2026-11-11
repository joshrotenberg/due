use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io::ErrorKind as IoErrorKind,
    sync::Arc,
    time::Duration,
};

use claude_wrapper::streaming::{
    BlockType, PartialMessageEvent, StreamEvent, stream_query_cancellable,
};
use claude_wrapper::{
    Claude, ClaudeCommand, Error, OutputFormat, QueryCommand, QueryResult, TerminationSignal,
};
use serde_json::Value;

use super::{ActivityKind, ActivityStatus, Context, Progress, RunOptions, output};
use crate::{
    result::{
        ErrorKind, PermissionDenial, PermissionDenials, ReportedSettings, RunError, RunResult,
        Usage,
    },
    types::{Effort, Provider},
};

pub async fn execute<C>(
    run: &RunOptions,
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
        .output_format(OutputFormat::Json)
        .verbose(true);
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
    if let Some(schema) = &context.schema {
        command = command.json_schema(schema.json());
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
        .working_dir(&context.cwd)
        .output_limit(output::STREAM_LIMIT)
        // Let the native CLI stop tool processes in their own groups before
        // the wrapper forces the remaining owned group to stop.
        .termination_signal(TerminationSignal::Interrupt)
        .kill_grace(Duration::from_secs(2));
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
            result.diagnostics = merge_diagnostics(&result.diagnostics, &output.stderr);
            result
        }
        Err(error) => failure(error),
    }
}

// Keep only the final response and bounded tool-correlation metadata, never a
// transcript. A content-block stop is not evidence that its tool has finished.
#[derive(Default)]
struct StreamState {
    reported: ReportedSettings,
    terminal: Option<Value>,
    invalid_terminal: Option<Value>,
    segments: ResultSegments,
    progress: Option<Progress>,
    // Latest complete text-bearing top-level message, for failures without a
    // terminal result. This is neither a transcript nor native structured data.
    last_assistant: String,
    session_id: Option<String>,
    active_tools: HashMap<String, String>,
    completed_tools: VecDeque<String>,
    turn_started: bool,
}

impl StreamState {
    fn observe(&mut self, event: StreamEvent, progress: &Progress) {
        super::provenance::claude_message(&mut self.reported, &event.data);
        self.progress = Some(progress.clone());
        let root = event
            .data
            .get("parent_tool_use_id")
            .is_none_or(Value::is_null);
        if root && opens_continuation(&event.data) {
            self.segments.continuation = true;
        }
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
                            self.tool_started(Some(&id), Some(&name), None, progress);
                        }
                        BlockType::Thinking => progress.phase("thinking_started"),
                        BlockType::Text => progress.phase("response_started"),
                        BlockType::Other(_) => (),
                    }
                }
            }
            Some("assistant") => {
                let content = event.data.pointer("/message/content");
                let mut text = content
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let mut detail = content.and_then(Value::as_str);
                if let Some(blocks) = content.and_then(Value::as_array) {
                    for block in blocks {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                detail =
                                    detail.or_else(|| block.get("text").and_then(Value::as_str));
                                if let Some(block_text) = block.get("text").and_then(Value::as_str)
                                {
                                    text.push_str(block_text);
                                }
                            }
                            Some("tool_use") => self.tool_started(
                                block.get("id").and_then(Value::as_str),
                                block.get("name").and_then(Value::as_str),
                                block.get("input"),
                                progress,
                            ),
                            _ => (),
                        }
                    }
                }
                progress.assistant(text.len(), detail);
                // Native subagent messages are also top-level JSONL envelopes.
                // Only root-session text may become the caller's partial answer.
                if !text.is_empty()
                    && event
                        .data
                        .get("parent_tool_use_id")
                        .is_none_or(Value::is_null)
                {
                    self.last_assistant = text;
                }
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
                        self.tool_completed(block, progress);
                    }
                }
            }
            Some("result") if root => {
                if self.segments.admit(&event.data, Some(progress)) {
                    self.terminal = Some(event.data);
                    self.last_assistant.clear();
                } else if self.segments.count == 0 {
                    self.invalid_terminal = Some(event.data);
                }
            }
            _ => (),
        }
    }

    fn tool_started(
        &mut self,
        id: Option<&str>,
        name: Option<&str>,
        input: Option<&Value>,
        progress: &Progress,
    ) {
        const MAX_ACTIVE_TOOLS: usize = 1024;
        let id = id.filter(|id| !id.is_empty());
        let detail = progress
            .detail_enabled()
            .then(|| tool_detail(name, input))
            .flatten();
        if let Some(id) = id {
            if self.active_tools.contains_key(id) {
                if input.is_some()
                    && let Some(detail) = detail
                {
                    progress.activity_detail(Some(id), activity_kind(name), name, detail);
                }
                return;
            }
            if self.completed_tools.iter().any(|completed| completed == id)
                || self.active_tools.len() >= MAX_ACTIVE_TOOLS
            {
                return;
            }
            self.active_tools
                .insert(id.to_owned(), name.unwrap_or_default().to_owned());
        }
        progress.activity_started(
            id,
            activity_kind(name),
            name.filter(|name| !name.is_empty()),
            detail,
        );
    }

    fn tool_completed(&mut self, block: &Value, progress: &Progress) {
        const MAX_COMPLETED_TOOLS: usize = 1024;
        let id = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        if let Some(id) = id {
            if self.completed_tools.iter().any(|completed| completed == id) {
                return;
            }
            if self.completed_tools.len() == MAX_COMPLETED_TOOLS {
                self.completed_tools.pop_front();
            }
            self.completed_tools.push_back(id.to_owned());
        }
        let name = id.and_then(|id| self.active_tools.remove(id));
        let name = name.as_deref().filter(|name| !name.is_empty());
        // Native tool_result omits is_error on success. An unexpected field
        // type is unknown rather than a fabricated successful completion.
        let status = match block.get("is_error") {
            None | Some(Value::Bool(false)) => ActivityStatus::Succeeded,
            Some(Value::Bool(true)) => ActivityStatus::Failed,
            Some(_) => ActivityStatus::Unknown,
        };
        progress.activity_completed(id, activity_kind(name), status, name, None);
    }

    fn finish(self, outcome: claude_wrapper::Result<claude_wrapper::CommandOutput>) -> RunResult {
        let terminal = self
            .terminal
            .as_ref()
            .or(self.invalid_terminal.as_ref())
            .map(parse_result);
        let missing_terminal = self.terminal.is_none();
        let process_failed = outcome.is_err();
        let mut result = match outcome {
            Ok(output) => {
                let mut result = terminal.unwrap_or_else(|| {
                    RunResult::new(Provider::Claude)
                        .fail(RunError::protocol("Claude stream has no terminal result"))
                });
                result.diagnostics = super::output::diagnostic(&output.stderr);
                result
            }
            Err(error) => {
                let mut failed = failure(error);
                match terminal {
                    Some(mut result) => {
                        // Stream callbacks own valid JSON events; the wrapper's
                        // error stdout contains only non-JSON diagnostics. Merge
                        // the retained terminal result with the process failure.
                        let mut error = failed.error.take().expect("failure carries an error");
                        if let Some(terminal_error) = result.error.take()
                            && ((error.kind == ErrorKind::Provider
                                && terminal_error.kind != ErrorKind::Protocol)
                                || (matches!(
                                    terminal_error.kind,
                                    ErrorKind::MaxTurns | ErrorKind::MaxBudget
                                ) && matches!(
                                    error.kind,
                                    ErrorKind::Authentication
                                        | ErrorKind::MaxTurns
                                        | ErrorKind::MaxBudget
                                ))
                                || (error.kind == ErrorKind::Authentication
                                    && terminal_error.kind == ErrorKind::Authentication))
                        {
                            // Buffered execution classifies explicit cap results
                            // before stderr auth heuristics. Streaming must keep
                            // that precedence despite its empty error stdout.
                            let exit_code = error.provider_exit_code;
                            error = terminal_error;
                            error.provider_exit_code = exit_code;
                        }
                        result.session_id = result.session_id.or(failed.session_id);
                        result.cost_usd = result.cost_usd.or(failed.cost_usd);
                        result.usage = result.usage.or(failed.usage);
                        result.structured_output =
                            result.structured_output.or(failed.structured_output);
                        result.diagnostics = failed.diagnostics;
                        result.fail(error)
                    }
                    _ => failed,
                }
            }
        };
        if result.session_id.is_none()
            && (self.segments.count == 1
                || missing_terminal
                || result
                    .error
                    .as_ref()
                    .is_some_and(|error| error.kind == ErrorKind::Protocol))
        {
            result.session_id = self.session_id;
        }
        if missing_terminal && self.segments.error.is_none() && result.error.is_some() {
            result.text = self.last_assistant;
        }
        if let Some(mut error) = self.segments.final_error()
            && !result.error.as_ref().is_some_and(|current| {
                matches!(
                    current.kind,
                    ErrorKind::Cancelled
                        | ErrorKind::Timeout
                        | ErrorKind::Io
                        | ErrorKind::Protocol
                        | ErrorKind::Authentication
                        | ErrorKind::MaxTurns
                        | ErrorKind::MaxBudget
                ) || (process_failed && current.kind == ErrorKind::Provider)
            })
        {
            error.provider_exit_code = result
                .error
                .as_ref()
                .and_then(|current| current.provider_exit_code);
            result = result.fail(error);
        }
        if self.segments.count > 0 {
            result.usage = self.segments.usage;
            result.permission_denials = self.segments.permission_denials;
        }
        if self.terminal.is_some() && !self.segments.continuation {
            if result.error.is_none() {
                if let Some(progress) = &self.progress {
                    progress.phase("turn_completed");
                }
            } else if self.terminal.as_ref().is_some_and(|terminal| {
                parse_result(terminal).error.as_ref().is_some_and(|error| {
                    matches!(
                        error.kind,
                        ErrorKind::Provider
                            | ErrorKind::Authentication
                            | ErrorKind::MaxTurns
                            | ErrorKind::MaxBudget
                    )
                })
            }) && !result.error.as_ref().is_some_and(|error| {
                matches!(
                    error.kind,
                    ErrorKind::Protocol | ErrorKind::Cancelled | ErrorKind::Timeout | ErrorKind::Io
                )
            }) && let Some(progress) = &self.progress
            {
                progress.phase("turn_failed");
            }
        }
        if self.segments.count > 1 {
            result.diagnostics = merge_diagnostics(
                &format!("Claude root result segments: {}", self.segments.count),
                &result.diagnostics,
            );
        }
        result.provenance.reported.merge(self.reported);
        result
    }
}

fn tool_detail<'a>(name: Option<&'a str>, input: Option<&'a Value>) -> Option<&'a str> {
    match name {
        Some("Bash") => input
            .and_then(|input| input.get("command"))
            .and_then(Value::as_str),
        Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => input
            .and_then(|input| {
                input
                    .get("file_path")
                    .or_else(|| input.get("notebook_path"))
            })
            .and_then(Value::as_str)
            .or(name),
        _ => name,
    }
}

fn activity_kind(name: Option<&str>) -> ActivityKind {
    match name {
        Some("Bash") => ActivityKind::Command,
        Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => ActivityKind::FileChange,
        Some("WebSearch" | "WebFetch") => ActivityKind::WebSearch,
        Some("TodoWrite" | "TaskCreate" | "TaskUpdate" | "EnterPlanMode" | "ExitPlanMode") => {
            ActivityKind::PlanUpdate
        }
        Some(name) if name.starts_with("mcp__") => ActivityKind::McpCall,
        Some("") | None => ActivityKind::Unknown,
        Some(_) => ActivityKind::ToolCall,
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

fn merge_diagnostics(prefix: &str, stderr: &str) -> String {
    if prefix.is_empty() {
        output::diagnostic(stderr)
    } else if stderr.is_empty() {
        output::diagnostic(prefix)
    } else {
        output::diagnostic(&format!("{prefix}\n{stderr}"))
    }
}

fn parse_document(document: &Value) -> RunResult {
    let Value::Array(records) = document else {
        let mut result = parse_result(document);
        if !matches!(
            result.error.as_ref().map(|error| error.kind),
            Some(ErrorKind::Protocol)
        ) {
            collect_permission_denials(&mut result.permission_denials, document, None);
        }
        return result;
    };
    let mut segments = ResultSegments::default();
    let mut terminal = None;
    let mut invalid_terminal = None;
    let mut reported = ReportedSettings::default();
    for record in records {
        super::provenance::claude_message(&mut reported, record);
        if record
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
        {
            continue;
        }
        if record.get("type").and_then(Value::as_str) == Some("result") {
            if segments.admit(record, None) {
                terminal = Some(record);
            } else if segments.count == 0 {
                invalid_terminal = Some(record);
            }
        } else if opens_continuation(record) {
            segments.continuation = true;
        }
    }
    let mut result = terminal
        .or(invalid_terminal)
        .map(parse_result)
        .unwrap_or_else(|| {
            RunResult::new(Provider::Claude).fail(RunError::protocol(
                "Claude transcript has no terminal result",
            ))
        });
    if let Some(error) = segments.final_error()
        && !result.error.as_ref().is_some_and(|current| {
            matches!(
                current.kind,
                ErrorKind::Authentication | ErrorKind::MaxTurns | ErrorKind::MaxBudget
            )
        })
    {
        result = result.fail(error);
    }
    if segments.count > 0 {
        result.usage = segments.usage;
        result.permission_denials = segments.permission_denials;
    }
    if segments.count > 1 {
        result.diagnostics =
            output::diagnostic(&format!("Claude root result segments: {}", segments.count));
    }
    result.provenance.reported = reported;
    result
}

fn opens_continuation(record: &Value) -> bool {
    match record.get("type").and_then(Value::as_str) {
        Some("assistant" | "user") => true,
        Some("system") => record.get("subtype").and_then(Value::as_str) == Some("init"),
        _ => false,
    }
}

#[derive(Default)]
struct ResultSegments {
    count: u64,
    continuation: bool,
    usage: Option<Usage>,
    permission_denials: PermissionDenials,
    error: Option<RunError>,
}

impl ResultSegments {
    fn final_error(&self) -> Option<RunError> {
        self.error
            .as_ref()
            .map(|error| RunError::protocol(error.message.clone()))
            .or_else(|| {
                (self.count > 0 && self.continuation)
                    .then(|| RunError::protocol("Claude continuation has no terminal result"))
            })
    }

    fn admit(&mut self, value: &Value, progress: Option<&Progress>) -> bool {
        if self.count > 0 && !self.continuation {
            self.error.get_or_insert_with(|| {
                RunError::protocol("adjacent Claude root results without continuation")
            });
            return false;
        }
        let mut parsed = parse_result(value);
        if parsed
            .error
            .as_ref()
            .is_some_and(|error| error.kind == ErrorKind::Protocol)
        {
            self.error
                .get_or_insert_with(|| parsed.error.take().expect("invalid result has an error"));
            self.continuation = false;
            return false;
        }
        self.continuation = false;
        let count = match self.count.checked_add(1) {
            Some(count) => count,
            None => {
                self.error.get_or_insert_with(|| {
                    RunError::protocol("Claude result segment count overflow")
                });
                return false;
            }
        };
        let next = parsed.usage;
        if count == 1 {
            self.usage = next;
        } else if let (Some(usage), Some(next)) = (self.usage.take(), next) {
            let mut folded = Usage::default();
            let mut overflow = false;
            macro_rules! add_bucket {
                ($field:ident) => {
                    folded.$field = match (usage.$field, next.$field) {
                        (Some(a), Some(b)) => match a.checked_add(b) {
                            Some(sum) => Some(sum),
                            None => {
                                overflow = true;
                                Some(a)
                            }
                        },
                        _ => None,
                    };
                };
            }
            add_bucket!(input_tokens);
            add_bucket!(cached_input_tokens);
            add_bucket!(cache_write_input_tokens);
            add_bucket!(output_tokens);
            add_bucket!(reasoning_output_tokens);
            add_bucket!(total_tokens);
            if overflow {
                self.error
                    .get_or_insert_with(|| RunError::protocol("Claude usage counter overflow"));
                self.usage = Some(usage);
                return false;
            }
            self.usage = Some(folded);
        } else {
            self.usage = None;
        }
        self.count = count;
        collect_permission_denials(&mut self.permission_denials, value, progress);
        true
    }
}

fn collect_permission_denials(
    summary: &mut PermissionDenials,
    terminal: &Value,
    progress: Option<&Progress>,
) {
    const LIMIT: usize = 64;
    let Some(native) = terminal.get("permission_denials") else {
        return;
    };
    let Some(entries) = native.as_array() else {
        summary.incomplete = true;
        return;
    };
    summary.incomplete |= entries.len() > LIMIT;
    for entry in entries.iter().take(LIMIT) {
        let (Some(tool_name), Some(tool_use_id)) = (
            entry
                .get("tool_name")
                .and_then(Value::as_str)
                .filter(|name| valid_denial_identifier(name)),
            entry
                .get("tool_use_id")
                .and_then(Value::as_str)
                .filter(|id| valid_denial_identifier(id)),
        ) else {
            summary.incomplete = true;
            continue;
        };
        if summary
            .items
            .iter()
            .any(|item| item.tool_use_id == tool_use_id)
        {
            continue;
        }
        if summary.items.len() == LIMIT {
            summary.incomplete = true;
            break;
        }
        summary.items.push(PermissionDenial {
            tool_name: tool_name.into(),
            tool_use_id: tool_use_id.into(),
        });
        if let Some(progress) = progress.filter(|progress| progress.detail_enabled())
            && tool_name == "Bash"
            && let Some(command) = entry.pointer("/tool_input/command").and_then(Value::as_str)
        {
            progress.permission_check_input(tool_use_id, command);
        }
    }
}

fn valid_denial_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:/".contains(&byte))
}

fn parse_result(value: &Value) -> RunResult {
    let result = RunResult::new(Provider::Claude);
    // Preserve the native value independently of readable text, even when the
    // terminal record reports failure or cannot otherwise be normalized.
    let result = RunResult {
        structured_output: value.get("structured_output").cloned(),
        ..result
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
        let kind = match value.get("subtype").and_then(Value::as_str) {
            Some("error_max_turns") => ErrorKind::MaxTurns,
            Some("error_max_budget_usd") => ErrorKind::MaxBudget,
            // Match native terminal diagnostics exactly. A tool or model
            // error can mention login without indicating a credential failure.
            _ if [
                "Failed to authenticate: OAuth session expired and could not be refreshed",
                "Not logged in · Please run /login",
            ]
            .iter()
            .any(|diagnostic| message.trim().eq_ignore_ascii_case(diagnostic)) =>
            {
                ErrorKind::Authentication
            }
            _ => ErrorKind::Provider,
        };
        return result.fail(RunError::new(kind, message));
    }
    if result.text.is_empty() {
        if let Some(structured) = &result.structured_output {
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
            let native_message = output::diagnostic(&stderr);
            if !matches!(
                parsed.error.as_ref().map(|error| error.kind),
                Some(ErrorKind::Protocol)
            ) {
                result = parsed;
            } else {
                result.text = parsed.text;
                result.session_id = parsed.session_id;
                result.cost_usd = parsed.cost_usd;
                result.usage = parsed.usage;
                result.structured_output = parsed.structured_output;
                result.permission_denials = parsed.permission_denials;
                result.provenance = parsed.provenance;
                result.diagnostics = parsed.diagnostics;
            }
            result.diagnostics = merge_diagnostics(&result.diagnostics, &stderr);
            result
                .error
                .take()
                .unwrap_or_else(|| {
                    let message = if native_message.trim().is_empty() {
                        format!("Claude exited with status {exit_code}")
                    } else {
                        native_message.trim().to_owned()
                    };
                    RunError::new(ErrorKind::Provider, message)
                })
                .with_exit_code(exit_code)
        }
        Error::Auth {
            kind,
            message,
            exit_code,
            ..
        } => {
            // A rate limit or exhausted quota needs waiting or credit, not a
            // new login. Only a native credential failure stays authentication.
            let rate_limited = kind == claude_wrapper::auth::AuthErrorKind::RateLimit;
            // The wrapper can classify stdout and retain it only as the auth
            // message. Recover a valid terminal result's text and metadata.
            let parsed = parse(&message);
            if parsed.error.as_ref().is_some_and(|error| {
                error.kind == ErrorKind::Authentication
                    || (rate_limited && error.kind != ErrorKind::Protocol)
            }) {
                result = parsed;
            }
            match result.error.take() {
                Some(error) if rate_limited => error,
                Some(error) => RunError::new(ErrorKind::Authentication, error.message),
                None if rate_limited => RunError::new(ErrorKind::Provider, message),
                None => RunError::new(ErrorKind::Authentication, message),
            }
            .with_exit_code(exit_code)
        }
        Error::MaxTurnsExceeded {
            session_id,
            cost_usd,
            exit_code,
            ..
        } => {
            result.session_id = session_id;
            result.cost_usd = cost_usd;
            RunError::new(ErrorKind::MaxTurns, "Claude reached its maximum turn count")
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
            RunError::new(ErrorKind::MaxBudget, "Claude reached its spending limit")
                .with_exit_code(exit_code)
        }
        Error::Json { source, .. } => RunError::protocol(format!("invalid Claude JSON: {source}")),
        unsupported @ (Error::VersionMismatch { .. } | Error::UntestedCliVersion { .. }) => {
            RunError::new(ErrorKind::Unsupported, unsupported.to_string())
        }
        configuration @ Error::DangerousNotAllowed { .. } => {
            RunError::new(ErrorKind::Configuration, configuration.to_string())
        }
        budget @ Error::BudgetExceeded { .. } => {
            RunError::new(ErrorKind::MaxBudget, budget.to_string())
        }
        Error::NotFound => RunError::new(ErrorKind::NotFound, "claude was not found on PATH"),
        Error::Timeout { timeout_seconds } => RunError::new(
            ErrorKind::Timeout,
            format!("Claude exceeded its execution timeout ({timeout_seconds}s)"),
        ),
        Error::Cancelled => RunError::new(ErrorKind::Cancelled, "Claude run cancelled"),
        Error::OutputLimitExceeded {
            stream,
            limit_bytes,
        } => RunError::new(
            ErrorKind::Protocol,
            format!("Claude {stream} exceeded the {limit_bytes}-byte captured stream limit"),
        ),
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

    #[test]
    fn permission_denials_are_bounded_and_omit_empty_evidence() {
        let mut native = json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "finished",
            "permission_denials": []
        });
        let envelope = |value: &Value| serde_json::to_value(parse_document(value)).unwrap();
        assert!(envelope(&native).get("permission_denials").is_none());

        native["permission_denials"] = json!([{
            "tool_name": "Bash",
            "tool_use_id": "malformed id",
            "tool_input": {"command": "private"}
        }]);
        assert_eq!(
            envelope(&native)["permission_denials"],
            json!({"items": [], "incomplete": true})
        );

        native["permission_denials"] = Value::Array(
            (0..65)
                .map(|i| json!({"tool_name": "Bash", "tool_use_id": format!("tool-{i}")}))
                .collect(),
        );
        let result = envelope(&native);
        assert_eq!(
            result["permission_denials"]["items"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(result["permission_denials"]["incomplete"], true);
    }

    #[test]
    fn provenance_survives_buffered_and_streamed_failures() {
        let process_error = |stdout| Error::CommandFailed {
            command: "fixture".into(),
            working_dir: None,
            exit_code: 9,
            stdout,
            stderr: "native diagnostic".into(),
        };
        let records = json!([
            {"type":"assistant","message":{"role":"assistant","model":"native-a","content":"partial"}},
            {"type":"assistant","message":{"role":"assistant","model":"native-b","content":"partial"}},
            {"type":"result","subtype":"error_during_execution","is_error":true,
             "result":"partial", "session_id":"native-id", "structured_output":{"partial":true},
             "usage":{"output_tokens":3}}
        ]);
        let buffered = failure(process_error(records.to_string()));
        let mut state = StreamState::default();
        for data in records.as_array().unwrap() {
            state.observe(
                StreamEvent { data: data.clone() },
                &(Arc::new(()) as Progress),
            );
        }
        let streamed = state.finish(Err(process_error(String::new())));
        for result in [buffered, streamed] {
            assert_eq!(result.provenance.reported.models, ["native-a", "native-b"]);
            assert!(result.provenance.reported.effort.is_none());
            assert_eq!(result.text, "partial");
            assert_eq!(result.session_id.as_deref(), Some("native-id"));
            assert_eq!(result.usage.unwrap().output_tokens, Some(3));
            assert_eq!(result.structured_output, Some(json!({"partial":true})));
            assert_eq!(result.error.unwrap().kind, ErrorKind::Provider);
        }

        let mut state = StreamState::default();
        state.observe(
            StreamEvent {
                data: records[0].clone(),
            },
            &(Arc::new(()) as Progress),
        );
        let result = state.finish(Err(Error::Cancelled));
        assert_eq!(result.provenance.reported.models, ["native-a"]);
        assert_eq!(result.text, "partial");
        assert_eq!(result.error.unwrap().kind, ErrorKind::Cancelled);

        // An incomplete transcript still retains observations on process failure.
        let result = failure(process_error(json!([records[0]]).to_string()));
        assert_eq!(result.provenance.reported.models, ["native-a"]);
        assert_eq!(result.error.unwrap().kind, ErrorKind::Provider);
    }

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

    fn expired_oauth_terminal() -> Value {
        json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true,
            "result": "partial answer",
            "errors": ["Failed to authenticate: OAuth session expired and could not be refreshed"],
            "session_id": "expired-session", "total_cost_usd": 0.03,
            "usage": {"input_tokens": 120, "output_tokens": 45},
            "structured_output": {"partial": true}
        })
    }

    fn assert_expired_oauth_result(result: RunResult) {
        assert_eq!(result.session_id.as_deref(), Some("expired-session"));
        assert_eq!(result.text, "partial answer");
        assert_eq!(result.structured_output, Some(json!({"partial": true})));
        assert_eq!(result.cost_usd, Some(0.03));
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(120));
        assert_eq!(usage.output_tokens, Some(45));
        let error = result.error.unwrap();
        assert_eq!(error.kind, ErrorKind::Authentication);
        assert_eq!(error.provider_exit_code, Some(9));
        assert_eq!(
            error.message,
            "Failed to authenticate: OAuth session expired and could not be refreshed"
        );
    }

    #[test]
    fn buffered_typed_auth_recovers_native_terminal_and_prefers_its_message() {
        let result = failure(Error::Auth {
            kind: claude_wrapper::auth::AuthErrorKind::Expired,
            command: "unused command".into(),
            exit_code: 9,
            message: expired_oauth_terminal().to_string(),
        });
        assert_expired_oauth_result(result);
    }

    #[test]
    fn streamed_typed_auth_prefers_native_terminal_message_and_metadata() {
        let state = StreamState {
            terminal: Some(expired_oauth_terminal()),
            ..StreamState::default()
        };
        let result = state.finish(Err(Error::Auth {
            kind: claude_wrapper::auth::AuthErrorKind::Expired,
            command: "unused command".into(),
            exit_code: 9,
            message: "wrapper auth diagnostic".into(),
        }));
        assert_expired_oauth_result(result);
    }

    #[test]
    fn native_oauth_expiry_requires_the_complete_failure_message() {
        const EXPIRY: &str =
            "Failed to authenticate: OAuth session expired and could not be refreshed";
        for (name, subtype, is_error, text, errors, expected_kind, expected_message) in [
            (
                "marker only in errors",
                "error_during_execution",
                true,
                "partial answer",
                Some(vec![EXPIRY]),
                Some(ErrorKind::Authentication),
                Some(EXPIRY),
            ),
            (
                "errors precede result text",
                "error_during_execution",
                true,
                EXPIRY,
                Some(vec!["unrelated provider failure"]),
                Some(ErrorKind::Provider),
                Some("unrelated provider failure"),
            ),
            (
                "mixed case",
                "error_during_execution",
                true,
                "partial answer",
                Some(vec![
                    "fAiLeD tO aUtHeNtIcAtE: oAuTh SeSsIoN eXpIrEd AnD cOuLd NoT bE rEfReShEd",
                ]),
                Some(ErrorKind::Authentication),
                Some("fAiLeD tO aUtHeNtIcAtE: oAuTh SeSsIoN eXpIrEd AnD cOuLd NoT bE rEfReShEd"),
            ),
            (
                "successful quoted text",
                "success",
                false,
                "The log says: Failed to authenticate: OAuth session expired and could not be refreshed",
                None,
                None,
                None,
            ),
            (
                "failed partial prose quoting expiry",
                "error_during_execution",
                true,
                "The log says: Failed to authenticate: OAuth session expired and could not be refreshed",
                None,
                Some(ErrorKind::Provider),
                Some(
                    "The log says: Failed to authenticate: OAuth session expired and could not be refreshed",
                ),
            ),
            (
                "unrelated failure",
                "error_during_execution",
                true,
                "partial answer",
                Some(vec!["unrelated provider failure"]),
                Some(ErrorKind::Provider),
                Some("unrelated provider failure"),
            ),
            (
                "cap precedes expiry",
                "error_max_budget_usd",
                true,
                "partial answer",
                Some(vec![EXPIRY]),
                Some(ErrorKind::MaxBudget),
                Some(EXPIRY),
            ),
        ] {
            let native = json!({
                "type": "result", "subtype": subtype, "is_error": is_error,
                "result": text, "errors": errors, "session_id": "expired-session",
                "total_cost_usd": 0.03,
                "usage": {"input_tokens": 120, "output_tokens": 45},
                "structured_output": {"partial": true}
            });
            let result = parse_document(&native);
            assert_eq!(result.text, text, "{name}");
            assert_eq!(
                result.session_id.as_deref(),
                Some("expired-session"),
                "{name}"
            );
            assert_eq!(result.cost_usd, Some(0.03), "{name}");
            assert_eq!(
                result.structured_output,
                Some(json!({"partial": true})),
                "{name}"
            );
            let usage = result.usage.unwrap();
            assert_eq!(usage.input_tokens, Some(120), "{name}");
            assert_eq!(usage.output_tokens, Some(45), "{name}");
            assert_eq!(
                result.error.as_ref().map(|error| error.kind),
                expected_kind,
                "{name}"
            );
            assert_eq!(
                result.error.as_ref().map(|error| error.message.as_str()),
                expected_message,
                "{name}"
            );
        }
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
        state.observe(
            StreamEvent {
                data: final_result.clone(),
            },
            &(Arc::new(()) as Progress),
        );
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

    #[test]
    fn structured_output_is_independent_of_readable_text_and_failure() {
        for is_error in [false, true] {
            let native = json!({
                "type": "result", "is_error": is_error, "result": "A readable answer",
                "structured_output": {"answer": 42, "ok": true}
            });
            let result = parse_document(&native);
            assert_eq!(result.text, "A readable answer");
            assert_eq!(
                result.structured_output,
                Some(json!({"answer": 42, "ok": true}))
            );
            assert_eq!(result.error.is_some(), is_error);
        }
        let malformed = json!({
            "type": "result", "is_error": "invalid", "structured_output": [1, 2, 3]
        });
        let result = StreamState {
            terminal: Some(malformed),
            ..StreamState::default()
        }
        .finish(Err(Error::Cancelled));
        assert_eq!(result.error.unwrap().kind, ErrorKind::Cancelled);
        assert_eq!(result.structured_output, Some(json!([1, 2, 3])));
    }

    #[test]
    fn explicit_structured_null_is_a_value_and_missing_output_is_a_protocol_error() {
        let result = parse_document(&json!({
            "type": "result", "is_error": false, "structured_output": null
        }));
        assert!(result.error.is_none());
        assert_eq!(result.structured_output, Some(Value::Null));
        assert_eq!(result.text, "null");
        let missing = parse_document(&json!({"type": "result", "is_error": false}));
        assert_eq!(missing.error.unwrap().kind, ErrorKind::Protocol);
        assert!(missing.structured_output.is_none());
    }

    #[test]
    fn native_terminal_failure_kinds_survive_buffered_and_streamed_process_failures() {
        for (subtype, expected) in [
            ("error_max_turns", ErrorKind::MaxTurns),
            ("error_max_budget_usd", ErrorKind::MaxBudget),
            ("error_during_execution", ErrorKind::Provider),
            ("error_future_subtype", ErrorKind::Provider),
        ] {
            let mut native = terminal();
            native["subtype"] = json!(subtype);
            native["structured_output"] = json!({"partial": true});
            let process_error = |stdout: String| Error::CommandFailed {
                command: "unused command".into(),
                exit_code: 9,
                stdout,
                stderr: "native diagnostic".into(),
                working_dir: None,
            };
            let buffered = failure(process_error(native.to_string()));
            let streamed = StreamState {
                terminal: Some(native),
                ..StreamState::default()
            }
            .finish(Err(process_error(String::new())));
            for result in [buffered, streamed] {
                let error = result.error.unwrap();
                assert_eq!(error.kind, expected);
                assert_eq!(error.provider_exit_code, Some(9));
                assert_eq!(result.session_id.as_deref(), Some("session-1"));
                assert_eq!(result.structured_output, Some(json!({"partial": true})));
            }
        }
    }

    #[test]
    fn wrapper_typed_errors_keep_their_categories() {
        for (stdout, stderr, expected) in [
            (
                r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#,
                "",
                ErrorKind::MaxTurns,
            ),
            (
                r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true}"#,
                "",
                ErrorKind::MaxBudget,
            ),
            (
                "",
                "Not logged in. Please run /login",
                ErrorKind::Authentication,
            ),
        ] {
            let error = Error::from_command_failure(
                "unused command".into(),
                1,
                stdout.into(),
                stderr.into(),
                None,
            );
            let result = failure(error);
            let error = result.error.unwrap();
            assert_eq!(error.kind, expected);
            assert_eq!(error.provider_exit_code, Some(1));
        }
    }

    #[test]
    fn wrapper_rate_limit_is_a_provider_failure_and_credential_kinds_stay_authentication() {
        use claude_wrapper::auth::AuthErrorKind;
        for (kind, expected) in [
            (AuthErrorKind::RateLimit, ErrorKind::Provider),
            (AuthErrorKind::NotAuthenticated, ErrorKind::Authentication),
            (AuthErrorKind::Expired, ErrorKind::Authentication),
            (AuthErrorKind::InvalidCredentials, ErrorKind::Authentication),
            (AuthErrorKind::ProviderError, ErrorKind::Authentication),
            (AuthErrorKind::Other, ErrorKind::Authentication),
        ] {
            let typed = || Error::Auth {
                kind,
                command: "unused command".into(),
                exit_code: 7,
                message: "native diagnostic".into(),
            };
            let buffered = failure(typed());
            let streamed = StreamState::default().finish(Err(typed()));
            for result in [buffered, streamed] {
                let error = result.error.unwrap();
                assert_eq!(error.kind, expected, "{kind:?}");
                assert_eq!(error.message, "native diagnostic", "{kind:?}");
                assert_eq!(error.provider_exit_code, Some(7), "{kind:?}");
            }
        }
    }

    #[test]
    fn native_rate_limit_stderr_and_credential_stderr_map_to_their_own_kinds() {
        use claude_wrapper::auth::AuthErrorKind;
        for (stderr, wrapper_kind, expected) in [
            (
                "API Error: 429 Too Many Requests",
                AuthErrorKind::RateLimit,
                ErrorKind::Provider,
            ),
            (
                "Usage quota exceeded",
                AuthErrorKind::RateLimit,
                ErrorKind::Provider,
            ),
            (
                "Rate limit reached for requests",
                AuthErrorKind::RateLimit,
                ErrorKind::Provider,
            ),
            (
                "Not logged in. Please run /login",
                AuthErrorKind::NotAuthenticated,
                ErrorKind::Authentication,
            ),
            (
                "Invalid API key",
                AuthErrorKind::InvalidCredentials,
                ErrorKind::Authentication,
            ),
            (
                "OAuth token expired",
                AuthErrorKind::Expired,
                ErrorKind::Authentication,
            ),
        ] {
            let native = || {
                Error::from_command_failure(
                    "unused command".into(),
                    2,
                    String::new(),
                    format!("{stderr}\n"),
                    None,
                )
            };
            assert!(
                matches!(native(), Error::Auth { kind, .. } if kind == wrapper_kind),
                "{stderr}"
            );
            let buffered = failure(native());
            let streamed = StreamState::default().finish(Err(native()));
            for result in [buffered, streamed] {
                let error = result.error.unwrap();
                assert_eq!(error.kind, expected, "{stderr}");
                assert_eq!(error.message, stderr, "{stderr}");
                assert_eq!(error.provider_exit_code, Some(2), "{stderr}");
            }
        }
    }

    #[test]
    fn rate_limit_recovers_native_terminal_and_keeps_expired_oauth_authentication() {
        let mut quota = terminal();
        quota["errors"] = json!(["Usage quota exceeded"]);
        let typed = |message: String| Error::Auth {
            kind: claude_wrapper::auth::AuthErrorKind::RateLimit,
            command: "unused command".into(),
            exit_code: 9,
            message,
        };
        let buffered = failure(typed(quota.to_string()));
        let streamed = StreamState {
            terminal: Some(quota),
            ..StreamState::default()
        }
        .finish(Err(typed("API Error: 429".into())));
        for result in [buffered, streamed] {
            assert_eq!(result.session_id.as_deref(), Some("session-1"));
            assert_eq!(result.text, "partial answer");
            assert_eq!(result.cost_usd, Some(0.03));
            let error = result.error.unwrap();
            assert_eq!(error.kind, ErrorKind::Provider);
            assert_eq!(error.message, "Usage quota exceeded");
            assert_eq!(error.provider_exit_code, Some(9));
        }

        assert_expired_oauth_result(failure(typed(expired_oauth_terminal().to_string())));
        assert_expired_oauth_result(
            StreamState {
                terminal: Some(expired_oauth_terminal()),
                ..StreamState::default()
            }
            .finish(Err(typed("API Error: 429".into()))),
        );
    }

    #[test]
    fn streamed_cap_cancel_and_timeout_precede_rate_limit() {
        let mut native = terminal();
        native["subtype"] = json!("error_max_turns");
        let error = Error::from_command_failure(
            "unused command".into(),
            1,
            String::new(),
            "API Error: 429 Too Many Requests".into(),
            None,
        );
        let result = StreamState {
            terminal: Some(native),
            ..StreamState::default()
        }
        .finish(Err(error));
        let error = result.error.unwrap();
        assert_eq!(error.kind, ErrorKind::MaxTurns);
        assert_eq!(error.provider_exit_code, Some(1));
        assert_eq!(result.session_id.as_deref(), Some("session-1"));

        let mut limited = terminal();
        limited["errors"] = json!(["API Error: 429 Too Many Requests"]);
        for (error, expected) in [
            (Error::Cancelled, ErrorKind::Cancelled),
            (Error::Timeout { timeout_seconds: 5 }, ErrorKind::Timeout),
        ] {
            let result = StreamState {
                terminal: Some(limited.clone()),
                ..StreamState::default()
            }
            .finish(Err(error));
            assert_eq!(result.error.unwrap().kind, expected);
        }
    }

    #[test]
    fn explicit_streamed_cap_result_precedes_auth_like_stderr() {
        let mut native = terminal();
        native["subtype"] = json!("error_max_turns");
        let state = StreamState {
            terminal: Some(native),
            ..StreamState::default()
        };
        let error = Error::from_command_failure(
            "unused command".into(),
            1,
            String::new(),
            "Not logged in. Please run /login".into(),
            None,
        );
        assert!(matches!(error, Error::Auth { .. }));
        let result = state.finish(Err(error));
        let error = result.error.unwrap();
        assert_eq!(error.kind, ErrorKind::MaxTurns);
        assert_eq!(error.provider_exit_code, Some(1));
        assert_eq!(result.session_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn partial_terminal_does_not_erase_metadata_carried_by_a_typed_failure() {
        let state = StreamState {
            terminal: Some(json!({
                "type": "result", "subtype": "error_max_budget_usd", "is_error": true
            })),
            ..StreamState::default()
        };
        let error = Error::from_command_failure(
            "unused command".into(),
            1,
            json!({
                "type": "result", "subtype": "error_max_budget_usd", "is_error": true,
                "session_id": "session-carried-by-error", "total_cost_usd": 0.25
            })
            .to_string(),
            String::new(),
            None,
        );
        let result = state.finish(Err(error));
        assert_eq!(result.error.unwrap().kind, ErrorKind::MaxBudget);
        assert_eq!(
            result.session_id.as_deref(),
            Some("session-carried-by-error")
        );
        assert_eq!(result.cost_usd, Some(0.25));
    }

    #[test]
    fn known_tool_names_share_activity_kinds_without_inspecting_inputs() {
        for (tool, expected) in [
            (Some("Bash"), ActivityKind::Command),
            (Some("Edit"), ActivityKind::FileChange),
            (Some("mcp__fixture__lookup"), ActivityKind::McpCall),
            (Some("WebSearch"), ActivityKind::WebSearch),
            (Some("TodoWrite"), ActivityKind::PlanUpdate),
            (Some("MysteryFutureTool"), ActivityKind::ToolCall),
            (None, ActivityKind::Unknown),
        ] {
            assert_eq!(activity_kind(tool), expected);
        }
    }
}
