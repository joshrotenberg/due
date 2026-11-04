use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use claude_wrapper::{
    Claude, TerminationSignal,
    duplex::{DuplexOptions, DuplexSession, InboundEvent, PermissionDecision, PermissionHandler},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{EventSink, MAX_EVENT_BYTES, MAX_EVENTS, OpenOptions};
use crate::{
    result::{ErrorKind, RunError, RunResult, Usage},
    types::Provider,
};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
type Reply = oneshot::Sender<Result<(), String>>;
type Pending = Arc<Mutex<HashSet<String>>>;

enum Command {
    Start {
        id: String,
        prompt: String,
        reply: Reply,
    },
    Interrupt {
        id: String,
        reply: Reply,
    },
    Respond {
        id: String,
        response: Value,
        expected: Option<(String, u64)>,
        reply: Reply,
    },
    Close,
}

pub struct Backend {
    commands: mpsc::Sender<Command>,
    task: JoinHandle<Result<(), String>>,
}

impl Backend {
    pub async fn open(options: &OpenOptions, sink: EventSink) -> Result<Self, String> {
        let deadline = options.timeout_duration()?;
        let pending = Pending::default();
        let active = Arc::new(AtomicBool::new(false));
        let handler = {
            let pending = pending.clone();
            let active = active.clone();
            let sink = sink.clone();
            PermissionHandler::new(move |request| {
                let deny = |message: &str| {
                    sink.item(
                        "native_request",
                        json!({
                            "provider":"claude", "method":"can_use_tool",
                            "request_id":request.request_id, "tool_name":request.tool_name,
                            "input":request.input, "request":request.raw,
                        }),
                        None,
                    );
                    PermissionDecision::Deny {
                        message: message.into(),
                    }
                };
                let decision = if !active.load(Ordering::Acquire) {
                    deny("No active Due turn")
                } else {
                    let mut ids = pending.lock().unwrap();
                    if ids.len() >= 32 {
                        deny("Too many pending permission requests")
                    } else {
                        ids.insert(request.request_id.clone());
                        sink.interaction(
                            request.request_id,
                            json!({
                                "provider": "claude", "method": "can_use_tool",
                                "tool_name": request.tool_name, "input": request.input,
                                "request": request.raw,
                            }),
                        );
                        PermissionDecision::Defer
                    }
                };
                async move { decision }
            })
        };
        let mut client = Claude::builder()
            .termination_signal(TerminationSignal::Interrupt)
            .kill_grace(Duration::from_secs(2));
        if let Some(binary) = &options.binary {
            client = client.binary(binary);
        }
        if let Some(cwd) = &options.cwd {
            client = client.working_dir(cwd);
        }
        let client = client.build().map_err(|e| e.to_string())?;
        let mut opts = DuplexOptions::default()
            .on_permission(handler)
            .arg("--include-partial-messages");
        if let Some(model) = &options.model {
            opts = opts.model(model);
        }
        if let Some(effort) = options.effort {
            opts = opts.effort(match effort {
                crate::types::Effort::Low => claude_wrapper::Effort::Low,
                crate::types::Effort::Medium => claude_wrapper::Effort::Medium,
                crate::types::Effort::High => claude_wrapper::Effort::High,
            });
        }
        if let Some(id) = &options.resume {
            opts = opts.resume(id);
        }
        if options.ephemeral {
            opts = opts.no_session_persistence();
        }
        for argument in &options.native_args {
            opts = opts.arg(argument);
        }
        let session = DuplexSession::spawn(&client, opts)
            .await
            .map_err(|e| e.to_string())?;
        let events = session.subscribe();
        if let Some(id) = &options.resume {
            sink.native_session(id.clone());
        }
        let (commands, receiver) = mpsc::channel(16);
        let task = tokio::spawn(drive(
            session, events, receiver, sink, pending, active, deadline,
        ));
        Ok(Self { commands, task })
    }

    pub async fn start(&mut self, turn_id: String, prompt: String) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Start {
                id: turn_id,
                prompt,
                reply,
            })
            .await
            .map_err(|_| "Claude session is closed")?;
        result.await.map_err(|_| "Claude session is closed")?
    }

    pub fn enqueue_interrupt(&self, turn_id: &str) -> Result<super::InterruptAnswer, String> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .try_send(Command::Interrupt {
                id: turn_id.to_owned(),
                reply,
            })
            .map_err(|error| format!("Cannot enqueue Claude interruption: {error}"))?;
        Ok(answer)
    }

    pub async fn reply(
        &mut self,
        request_id: &str,
        response: Value,
        expected: Option<(String, u64)>,
    ) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Respond {
                id: request_id.into(),
                response,
                expected,
                reply,
            })
            .await
            .map_err(|_| "Claude session is closed")?;
        result.await.map_err(|_| "Claude session is closed")?
    }

    pub async fn close(self) -> Result<(), String> {
        let _ = self.commands.send(Command::Close).await;
        drop(self.commands);
        self.task
            .await
            .map_err(|e| format!("Claude session task failed: {e}"))?
    }
}

async fn drive(
    session: DuplexSession,
    mut events: broadcast::Receiver<InboundEvent>,
    mut commands: mpsc::Receiver<Command>,
    sink: EventSink,
    pending: Pending,
    active: Arc<AtomicBool>,
    deadline: Option<Duration>,
) -> Result<(), String> {
    let mut deferred_result = None;
    let mut failure = None;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Start { id, prompt, reply }) => {
                    // Anything already queued arrived before this turn was submitted.
                    loop {
                        match events.try_recv() {
                            Ok(event) => forward(Ok(event), &sink, None),
                            Err(broadcast::error::TryRecvError::Lagged(count)) => forward(Err(broadcast::error::RecvError::Lagged(count)), &sink, None),
                            _ => break,
                        }
                    }
                    active.store(true, Ordering::Release);
                    let _ = reply.send(Ok(()));
                    let outcome = drive_turn(&session, &mut events, &mut commands, &sink, &pending, id, prompt, deadline).await;
                    active.store(false, Ordering::Release);
                    pending.lock().unwrap().clear();
                    match outcome {
                        TurnEnd::Complete(result) => sink.finished(result),
                        TurnEnd::Close(result, reason) => {
                            deferred_result = Some(result);
                            failure = reason;
                            break;
                        }
                    }
                }
                Some(Command::Interrupt { reply, .. }) => { let _ = reply.send(Err("No active turn".into())); }
                Some(Command::Respond { reply, .. }) => { let _ = reply.send(Err("No pending interaction".into())); }
                Some(Command::Close) | None => break,
            },
            event = events.recv() => {
                // Late post-turn notifications must not enter a newly reserved turn.
                forward(event, &sink, None);
            },
            status = session.wait_for_exit() => {
                failure = Some(format!("Claude session exited: {status:?}"));
                break;
            }
        }
    }
    active.store(false, Ordering::Release);
    pending.lock().unwrap().clear();
    let closed = session.close().await.map_err(|e| e.to_string());
    // A deadline or explicit close is terminal only after the child has settled.
    if let Some(result) = deferred_result {
        sink.finished(result);
    }
    if let Some(reason) = failure {
        sink.failed(reason);
    }
    if let Err(error) = &closed {
        sink.failed(error.clone());
    }
    closed
}

enum TurnEnd {
    Complete(RunResult),
    Close(RunResult, Option<String>),
}

#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    session: &DuplexSession,
    events: &mut broadcast::Receiver<InboundEvent>,
    commands: &mut mpsc::Receiver<Command>,
    sink: &EventSink,
    pending: &Pending,
    id: String,
    prompt: String,
    deadline: Option<Duration>,
) -> TurnEnd {
    let started = Instant::now();
    let turn = session.send(prompt);
    tokio::pin!(turn);
    let timeout = async {
        match deadline {
            Some(duration) => tokio::time::sleep(duration).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(timeout);
    let mut interrupted = false;
    let mut interrupt_deadline = None;
    let end = loop {
        tokio::select! {
            biased;
            result = &mut turn => {
                // Drain already-delivered events before publishing the terminal result.
                loop {
                    match events.try_recv() {
                        Ok(event) => forward(Ok(event), sink, Some(&id)),
                        Err(broadcast::error::TryRecvError::Lagged(count)) => forward(Err(broadcast::error::RecvError::Lagged(count)), sink, Some(&id)),
                        _ => break,
                    }
                }
                match result {
                    Ok(turn) => {
                        if let Some(session_id) = turn.session_id() { sink.native_session(session_id.into()); }
                        // The live wrapper reduces init to its session ID. Its retained
                        // events can still supply the complete init after this turn.
                        for event in &turn.events {
                            if event.get("type").and_then(Value::as_str) == Some("system")
                                && event.get("subtype").and_then(Value::as_str) == Some("init") {
                                sink.item("native_snapshot", json!({"provider":"claude", "event":event}), Some(&id));
                            }
                        }
                        sink.item("native", json!({"provider":"claude", "event":turn.result}), Some(&id));
                        sink.event(json!({"provider":"claude", "event":turn.result}));
                        let result = normalize_turn(turn.result, &turn.events, interrupted);
                        break TurnEnd::Complete(result);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        break TurnEnd::Close(RunResult::new(Provider::Claude).fail(RunError::protocol(&message)), Some(message));
                    }
                }
            }
            () = &mut timeout => break TurnEnd::Close(
                RunResult::new(Provider::Claude).fail(RunError::new(ErrorKind::Timeout, "Claude turn deadline exceeded")),
                Some("Claude session closed after turn deadline".into()),
            ),
            () = async {
                match interrupt_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => break TurnEnd::Close(
                RunResult::new(Provider::Claude).fail(RunError::protocol("Claude acknowledged interruption but did not finish the turn")),
                Some("Claude session closed after interrupted turn failed to settle".into()),
            ),
            command = commands.recv() => match command {
                Some(Command::Start { reply, .. }) => { let _ = reply.send(Err("A turn is already running".into())); }
                Some(Command::Respond { id: request_id, response, expected, reply }) => {
                    let result = respond(session, pending, &request_id, response.clone(), expected.as_ref(), sink);
                    if result.is_ok() {
                        sink.item("interaction_reply", json!({"request_id":request_id,"response":response}), Some(&id));
                        sink.resolved_generation(&request_id, expected.as_ref().map(|(_, cursor)| *cursor));
                    }
                    let _ = reply.send(result);
                }
                Some(Command::Interrupt { id: requested, reply }) => {
                    if requested != id {
                        let _ = reply.send(Err("Unknown or finished Due turn to interrupt".into()));
                        continue;
                    }
                    match tokio::time::timeout(CONTROL_TIMEOUT, session.interrupt()).await {
                        Ok(Ok(())) => {
                            sink.item("interrupt_requested", json!({}), Some(&id));
                            interrupted = true;
                            // An acknowledgement does not close the in-flight send.
                            // Keep observing it, with a bounded settlement wait.
                            interrupt_deadline.get_or_insert_with(|| tokio::time::Instant::now() + CONTROL_TIMEOUT);
                            let _ = reply.send(Ok(()));
                        }
                        result => {
                            let message = match result {
                                Ok(Err(error)) => error.to_string(),
                                Err(_) => "Claude did not acknowledge interruption".into(),
                                _ => unreachable!(),
                            };
                            let _ = reply.send(Err(message.clone()));
                            break TurnEnd::Close(RunResult::new(Provider::Claude).fail(RunError::protocol(&message)), Some(message));
                        }
                    }
                }
                Some(Command::Close) | None => break TurnEnd::Close(
                    RunResult::new(Provider::Claude).fail(RunError::new(ErrorKind::Cancelled, "Session closed during turn")), None,
                ),
            },
            event = events.recv() => forward(event, sink, Some(&id)),
        }
    };
    let stamp = |mut result: RunResult| {
        result.run_id = Some(id);
        result.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        result
    };
    match end {
        TurnEnd::Complete(result) => TurnEnd::Complete(stamp(result)),
        TurnEnd::Close(result, reason) => TurnEnd::Close(stamp(result), reason),
    }
}

fn forward(
    event: Result<InboundEvent, broadcast::error::RecvError>,
    sink: &EventSink,
    turn_id: Option<&str>,
) {
    let (kind, event) = match event {
        Ok(InboundEvent::SystemInit { session_id }) => {
            sink.native_session(session_id.clone());
            (
                "native_summary",
                json!({"type":"system", "subtype":"init", "session_id":session_id}),
            )
        }
        Ok(
            InboundEvent::Assistant(value)
            | InboundEvent::StreamEvent(value)
            | InboundEvent::User(value)
            | InboundEvent::Other(value),
        ) => ("native", value),
        Err(broadcast::error::RecvError::Lagged(count)) => (
            "native_summary",
            json!({"type":"events_lost", "count":count}),
        ),
        Err(broadcast::error::RecvError::Closed) => return,
    };
    if let Some(turn_id) = turn_id {
        sink.claude_provenance(turn_id, &event);
    }
    let item = json!({"provider":"claude", "event":event});
    sink.item(kind, item.clone(), turn_id);
    if turn_id.is_some() {
        sink.event(item);
    }
}

#[derive(Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case", deny_unknown_fields)]
enum Decision {
    Allow { updated_input: Option<Value> },
    Deny { message: String },
}

pub(crate) fn validate_response(value: &Value) -> Result<(), String> {
    let decision: Decision = serde_json::from_value(value.clone())
        .map_err(|error| format!("Invalid Claude permission response: {error}"))?;
    if let Decision::Allow {
        updated_input: Some(input),
    } = decision
        && !input.is_object()
    {
        return Err("updated_input must be an object".into());
    }
    Ok(())
}

fn respond(
    session: &DuplexSession,
    pending: &Pending,
    id: &str,
    response: Value,
    expected: Option<&(String, u64)>,
    sink: &EventSink,
) -> Result<(), String> {
    let decision = match serde_json::from_value::<Decision>(response)
        .map_err(|e| format!("Invalid Claude permission response: {e}"))?
    {
        Decision::Allow { updated_input } => {
            if updated_input
                .as_ref()
                .is_some_and(|input| !input.is_object())
            {
                return Err("updated_input must be an object".into());
            }
            PermissionDecision::Allow { updated_input }
        }
        Decision::Deny { message } => PermissionDecision::Deny { message },
    };
    let mut ids = pending.lock().unwrap();
    if expected.is_some_and(|(turn, cursor)| !sink.interaction_current(turn, id, *cursor)) {
        return Err("interaction is no longer current".into());
    }
    if !ids.contains(id) {
        return Err("Unknown or resolved interaction".into());
    }
    session
        .respond_to_permission(id, decision)
        .map_err(|e| e.to_string())?;
    ids.remove(id);
    Ok(())
}

#[derive(Default)]
struct PartialMessage<'a> {
    id: Option<&'a str>,
    last_text_block: Option<u64>,
}

/// Only used on the wrapper's completed, turn-local event list. Native events
/// remain untouched, including when the bounded text fallback is exhausted.
#[derive(Default)]
struct PartialText<'a> {
    messages: Vec<PartialMessage<'a>>,
    blocks: BTreeMap<(usize, u64), String>,
    stream: Option<usize>,
    bytes: usize,
}

impl<'a> PartialText<'a> {
    fn message(&mut self, id: Option<&'a str>) -> Option<usize> {
        if let Some(id) = id
            && let Some(index) = self
                .messages
                .iter()
                .position(|message| message.id == Some(id))
        {
            return Some(index);
        }
        if self.messages.len() == MAX_EVENTS {
            return None;
        }
        self.messages.push(PartialMessage {
            id,
            last_text_block: None,
        });
        Some(self.messages.len() - 1)
    }

    fn text(&mut self, message: usize, block: u64, text: &str, replace: bool) {
        // Empty snapshots must not erase text already supplied by deltas.
        if text.is_empty() {
            return;
        }
        let key = (message, block);
        if !self.blocks.contains_key(&key) && self.blocks.len() == MAX_EVENTS {
            return;
        }
        let value = self.blocks.entry(key).or_default();
        if replace {
            self.bytes -= value.len();
            *value = String::new();
        }
        let mut end = text.len().min(MAX_EVENT_BYTES - self.bytes);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        value.push_str(&text[..end]);
        self.bytes += end;
    }

    fn observe(&mut self, value: &'a Value) {
        // Subagent envelopes share the native stream but are not the root answer.
        if value
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
        {
            return;
        }
        match value["type"].as_str() {
            Some("stream_event") => {
                let event = &value["event"];
                if event["type"] == "message_start" {
                    self.stream = self.message(event["message"]["id"].as_str());
                    return;
                }
                if !matches!(
                    event["type"].as_str(),
                    Some("content_block_start" | "content_block_delta")
                ) {
                    return;
                }
                let Some(block) = event["index"].as_u64() else {
                    return;
                };
                let message = match self.stream {
                    Some(message) => message,
                    None => {
                        let Some(message) = self.message(None) else {
                            return;
                        };
                        self.stream = Some(message);
                        message
                    }
                };
                let content = if event["type"] == "content_block_start" {
                    &event["content_block"]
                } else {
                    &event["delta"]
                };
                if matches!(content["type"].as_str(), Some("text" | "text_delta"))
                    && let Some(text) = content["text"].as_str()
                {
                    self.messages[message].last_text_block = Some(block);
                    self.text(message, block, text, false);
                }
            }
            Some("assistant") => {
                let Some(content) = value["message"]["content"].as_array() else {
                    return;
                };
                let id = value["message"]["id"].as_str();
                let message = if let Some(stream) = self.stream
                    && (id.is_none() || self.messages[stream].id.is_none())
                {
                    self.messages[stream].id = id.or(self.messages[stream].id);
                    Some(stream)
                } else {
                    self.message(id)
                };
                let Some(message) = message else { return };
                for (index, block) in content.iter().enumerate() {
                    if block["type"] == "text"
                        && let Some(text) = block["text"].as_str()
                    {
                        // Claude can snapshot a single completed block, or the
                        // whole message. A streamed single block keeps its index.
                        let index = if content.len() == 1 {
                            self.messages[message].last_text_block.unwrap_or(0)
                        } else {
                            index as u64
                        };
                        self.text(message, index, text, true);
                    }
                }
            }
            _ => {}
        }
    }
}

fn partial_text(events: &[Value]) -> String {
    let mut text = PartialText::default();
    for event in events {
        text.observe(event);
    }
    text.blocks.into_values().collect()
}

fn normalize_turn(native: Value, events: &[Value], interrupted: bool) -> RunResult {
    let mut result = normalize(native);
    for event in events {
        crate::providers::provenance::claude_message(&mut result.provenance.reported, event);
    }
    if result.text.is_empty() && (interrupted || result.error.is_some()) {
        result.text = partial_text(events);
    }
    if interrupted {
        result = result.fail(RunError::new(ErrorKind::Cancelled, "Turn interrupted"));
    }
    result
}

fn normalize(native: Value) -> RunResult {
    let mut result = RunResult::new(Provider::Claude);
    result.text = native
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into();
    result.session_id = native
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    result.cost_usd = native
        .get("total_cost_usd")
        .or_else(|| native.get("cost_usd"))
        .and_then(Value::as_f64);
    result.structured_output = native.get("structured_output").cloned();
    if let Some(usage) = native.get("usage").filter(|usage| usage.is_object()) {
        result.usage = Some(Usage {
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
            cached_input_tokens: usage.get("cache_read_input_tokens").and_then(Value::as_u64),
            cache_write_input_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
            ..Usage::default()
        });
    }
    match (
        native.get("is_error").and_then(Value::as_bool),
        native.get("subtype").and_then(Value::as_str),
    ) {
        (Some(false), Some("success")) => result,
        (Some(_), Some(subtype)) => {
            let kind = match subtype {
                "error_max_turns" => ErrorKind::MaxTurns,
                "error_max_budget_usd" => ErrorKind::MaxBudget,
                _ => ErrorKind::Provider,
            };
            let message = native
                .get("errors")
                .map(Value::to_string)
                .unwrap_or_else(|| format!("Claude turn failed: {subtype}"));
            result.fail(RunError::new(kind, message))
        }
        _ => result.fail(RunError::protocol(
            "Claude result is missing is_error or subtype",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::Status;

    fn stream(event: Value) -> Value {
        json!({"type":"stream_event", "event":event})
    }

    fn start_message(id: &str) -> Value {
        stream(json!({"type":"message_start", "message":{"id":id}}))
    }

    fn text_delta(index: u64, text: &str) -> Value {
        stream(json!({"type":"content_block_delta", "index":index,
            "delta":{"type":"text_delta", "text":text}}))
    }

    fn assistant(id: &str, content: Value) -> Value {
        json!({"type":"assistant", "message":{"id":id, "content":content}})
    }

    #[test]
    fn partial_text_excludes_subagents_without_disrupting_root_stream() {
        let mut nested = [
            start_message("nested"),
            text_delta(0, "Nested delta"),
            assistant("nested", json!([{"type":"text", "text":"Nested answer"}])),
        ];
        for event in &mut nested {
            event["parent_tool_use_id"] = json!("tool-agent");
        }
        let mut events = vec![start_message("root"), text_delta(0, "Root ")];
        events.extend(nested);
        events.push(text_delta(0, "answer"));
        let mut snapshot = assistant("root", json!([{"type":"text", "text":"Root answer"}]));
        snapshot["parent_tool_use_id"] = Value::Null;
        events.push(snapshot);
        assert_eq!(partial_text(&events), "Root answer");
    }

    #[test]
    fn turn_fallback_preserves_successful_empty_answers_and_terminal_precedence() {
        let events = [assistant("one", json!([{"type":"text", "text":"Partial"}]))];
        let success = json!({"type":"result", "subtype":"success", "is_error":false, "result":""});
        assert_eq!(normalize_turn(success.clone(), &events, false).text, "");
        let cancelled = normalize_turn(success.clone(), &events, true);
        assert_eq!(cancelled.text, "Partial");
        assert!(matches!(cancelled.status, Status::Cancelled));
        let mut failed = success.clone();
        failed["subtype"] = json!("error_during_execution");
        failed["is_error"] = json!(true);
        let failed = normalize_turn(failed, &events, false);
        assert_eq!(failed.text, "Partial");
        assert!(matches!(failed.status, Status::Failed));
        let mut authoritative = success;
        authoritative["result"] = json!("Final answer");
        assert_eq!(
            normalize_turn(authoritative, &events, true).text,
            "Final answer"
        );
    }

    #[test]
    fn partial_text_keeps_all_assistant_messages_and_only_text_blocks() {
        let events = [
            assistant(
                "one",
                json!([
                    {"type":"text", "text":"same "},
                    {"type":"thinking", "thinking":"private", "text":"not text"},
                    {"type":"tool_use", "input":{"text":"tool input"}},
                    {"type":"text", "text":"same "}
                ]),
            ),
            json!({"type":"user", "message":{"content":[
                {"type":"tool_result", "content":"tool output"},
                {"type":"text", "text":"user text"}
            ]}}),
            assistant("two", json!([{"type":"text", "text":"same "}])),
        ];
        assert_eq!(partial_text(&events), "same same same ");
    }

    #[test]
    fn partial_text_single_block_snapshots_replace_only_their_deltas() {
        let events = [
            start_message("one"),
            text_delta(0, "draft one"),
            assistant("one", json!([{"type":"text", "text":"First. "}])),
            // Replayed snapshots for this block must not duplicate its text.
            assistant("one", json!([{"type":"text", "text":"First. "}])),
            stream(json!({"type":"content_block_start", "index":1,
                "content_block":{"type":"thinking", "thinking":"hidden"}})),
            stream(json!({"type":"content_block_delta", "index":1,
                "delta":{"type":"thinking_delta", "thinking":"hidden"}})),
            text_delta(2, "draft two"),
            assistant("one", json!([{"type":"text", "text":"Second. "}])),
            text_delta(3, "Unfinished. "),
            // A later empty snapshot must not discard available text.
            assistant("one", json!([{"type":"text", "text":""}])),
            start_message("two"),
            text_delta(0, "Next message."),
        ];
        assert_eq!(
            partial_text(&events),
            "First. Second. Unfinished. Next message."
        );
    }

    #[test]
    fn partial_text_cumulative_snapshot_replaces_each_streamed_block() {
        let events = [
            start_message("one"),
            text_delta(0, "draft one"),
            text_delta(2, "draft two"),
            assistant(
                "one",
                json!([
                    {"type":"text", "text":"same "},
                    {"type":"tool_use", "input":{"text":"not answer"}},
                    {"type":"text", "text":"same "}
                ]),
            ),
            start_message("two"),
            text_delta(0, "draft three"),
            assistant("two", json!([{"type":"text", "text":"same "}])),
        ];
        assert_eq!(partial_text(&events), "same same same ");
    }

    #[test]
    fn partial_text_delta_only_includes_block_starts_and_ignores_nontext_deltas() {
        let events = [
            // A missing message_start is tolerated for an otherwise valid delta.
            text_delta(0, "First. "),
            start_message("two"),
            stream(json!({"type":"content_block_start", "index":0,
                "content_block":{"type":"text", "text":"Second"}})),
            text_delta(0, ". "),
            stream(json!({"type":"content_block_delta", "index":1,
                "delta":{"type":"input_json_delta", "partial_json":"tool input"}})),
            text_delta(2, "Third."),
        ];
        assert_eq!(partial_text(&events), "First. Second. Third.");
    }

    #[test]
    fn partial_text_without_message_ids_still_replaces_streamed_text() {
        let events = [
            text_delta(0, "draft"),
            json!({"type":"assistant", "aborted":true,
                "message":{"content":[{"type":"text", "text":"Available."}]}}),
        ];
        assert_eq!(partial_text(&events), "Available.");
    }

    #[test]
    fn partial_text_uses_session_byte_and_item_limits_at_utf8_boundaries() {
        let prefix = "a".repeat(MAX_EVENT_BYTES - 1);
        let mut events = vec![
            start_message("one"),
            text_delta(0, &prefix),
            text_delta(0, "é"),
        ];
        assert_eq!(partial_text(&events), prefix);
        events.push(assistant(
            "one",
            json!([
                {"type":"text", "text":"é".repeat(MAX_EVENT_BYTES)}
            ]),
        ));
        assert_eq!(partial_text(&events), "é".repeat(MAX_EVENT_BYTES / 2));

        let blocks: Vec<_> = (0..MAX_EVENTS + 1)
            .map(|index| text_delta(u64::MAX - index as u64, "x"))
            .collect();
        assert_eq!(partial_text(&blocks), "x".repeat(MAX_EVENTS));
        let messages: Vec<_> = (0..MAX_EVENTS + 1)
            .map(|index| assistant(&index.to_string(), json!([{"type":"text", "text":"x"}])))
            .collect();
        assert_eq!(partial_text(&messages), "x".repeat(MAX_EVENTS));
    }

    #[test]
    fn native_failure_cannot_become_success_through_text_or_partial_data() {
        let result = normalize(json!({
            "type": "result", "subtype": "error_max_turns", "is_error": true,
            "result": "partial answer", "structured_output": {"partial": true},
            "session_id": "native-session", "total_cost_usd": 0.25,
        }));
        assert!(matches!(result.status, Status::Failed));
        assert!(matches!(result.error.unwrap().kind, ErrorKind::MaxTurns));
        assert_eq!(result.text, "partial answer");
        assert_eq!(result.session_id.as_deref(), Some("native-session"));
        assert_eq!(result.cost_usd, Some(0.25));
        assert!(result.structured_output.is_some());
    }

    #[test]
    fn missing_terminal_status_is_a_protocol_failure() {
        let result = normalize(json!({"result": "looks successful"}));
        assert!(matches!(result.error.unwrap().kind, ErrorKind::Protocol));
    }

    #[test]
    fn unknown_permission_fields_do_not_turn_into_approval() {
        assert!(
            serde_json::from_value::<Decision>(json!({
                "behavior": "allow", "updated_input": {}, "unexpected": true,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Decision>(json!({
                "behavior": "approve",
            }))
            .is_err()
        );
    }
}
