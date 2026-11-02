use std::{
    collections::HashSet,
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

use super::{EventSink, OpenOptions};
use crate::{
    cli::Provider,
    result::{ErrorKind, RunError, RunResult, Usage},
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
                crate::cli::Effort::Low => claude_wrapper::Effort::Low,
                crate::cli::Effort::Medium => claude_wrapper::Effort::Medium,
                crate::cli::Effort::High => claude_wrapper::Effort::High,
            });
        }
        if let Some(id) = &options.resume {
            opts = opts.resume(id);
        }
        if options.ephemeral {
            opts = opts.no_session_persistence();
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

    pub async fn reply(&mut self, request_id: &str, response: Value) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Respond {
                id: request_id.into(),
                response,
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
        sink.failed(reason.clone());
        return Err(reason);
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
                        let mut result = normalize(turn.result);
                        if interrupted { result = result.fail(RunError::new(ErrorKind::Cancelled, "Turn interrupted")); }
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
                Some(Command::Respond { id: request_id, response, reply }) => {
                    let result = respond(session, pending, &request_id, response.clone());
                    if result.is_ok() {
                        sink.item("interaction_reply", json!({"request_id":request_id,"response":response}), Some(&id));
                        sink.resolved(&request_id);
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

fn respond(
    session: &DuplexSession,
    pending: &Pending,
    id: &str,
    response: Value,
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
    if !ids.contains(id) {
        return Err("Unknown or resolved interaction".into());
    }
    session
        .respond_to_permission(id, decision)
        .map_err(|e| e.to_string())?;
    ids.remove(id);
    Ok(())
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
