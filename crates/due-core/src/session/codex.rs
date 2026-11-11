//! Resident Codex transport. The owner keeps reading while callers control a turn.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

use codex_wrapper::{
    Codex,
    app_server::{
        AppServer, AppServerHandle, Notification, RequestId, ServerMessage, ThreadStartParams,
        Turn, TurnStartParams, TurnStatus,
    },
};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{CloseError, EventSink, OpenOptions};
use crate::{
    result::{ErrorKind, RunError, RunResult, Usage},
    types::Provider,
};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 64 * 1024 * 1024;
const MAX_ANSWER: usize = 4 * 1024 * 1024;

type Reply = oneshot::Sender<Result<(), String>>;
type InterruptOperation = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

// One session-wide control slot. Dispatched acknowledgement ownership survives
// turn completion; pre-dispatch intent remains bound to its original Due turn.
struct PendingInterrupt {
    due_turn: String,
    native_id: Option<String>,
    reply: Option<Reply>,
    deadline: tokio::time::Instant,
    operation: Option<InterruptOperation>,
}

impl PendingInterrupt {
    fn new(due_turn: String, reply: Reply, deadline: tokio::time::Instant) -> Self {
        Self {
            due_turn,
            native_id: None,
            reply: Some(reply),
            deadline,
            operation: None,
        }
    }

    fn pending(&self) -> bool {
        self.reply.is_some()
    }

    fn reject(&mut self, message: &str) {
        self.operation = None;
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(Err(message.to_owned()));
        }
    }

    fn dispatch(
        &mut self,
        handle: AppServerHandle,
        thread: String,
        native: String,
        sink: EventSink,
    ) {
        if self.operation.is_some() || !self.pending() {
            return;
        }
        self.native_id = Some(native.clone());
        let due_turn = self.due_turn.clone();
        self.operation = Some(Box::pin(async move {
            handle
                .turn_interrupt(&thread, &native)
                .await
                .map_err(|error| {
                    let explicit_rejection = matches!(&error, codex_wrapper::Error::AppServerRpc { .. });
                    let message = report_rpc_error(error, &sink, Some(&due_turn));
                    if explicit_rejection { message } else {
                        format!("Codex interruption acknowledgement unknown; request may already have been delivered: {message}")
                    }
                })
        }));
    }
}

impl Drop for PendingInterrupt {
    fn drop(&mut self) {
        let message = if self.native_id.is_some() {
            "Codex control owner closed; interruption acknowledgement unknown, request may already have been delivered"
        } else {
            "Codex control owner closed before interruption dispatch"
        };
        self.reject(message);
    }
}

async fn interrupt_accepted(interrupt: &mut Option<PendingInterrupt>) -> Result<(), String> {
    match interrupt
        .as_mut()
        .and_then(|control| control.operation.as_mut())
    {
        Some(operation) => operation.await,
        None => std::future::pending().await,
    }
}

enum Command {
    Start {
        id: String,
        prompt: String,
        reply: Reply,
    },
    Started {
        id: String,
        result: Result<Value, String>,
    },
    Interrupt {
        id: String,
        deadline: tokio::time::Instant,
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
    owner: Option<JoinHandle<Result<(), CloseError>>>,
}

struct Active {
    id: String,
    native_id: Option<String>,
    answer: Option<String>,
    partial: Option<String>,
    usage: Option<Usage>,
    requests: HashMap<String, RequestId>,
    early: Vec<ServerMessage>,
    early_bytes: usize,
    early_requests: HashMap<String, Value>,
    started: Instant,
    deadline: Option<tokio::time::Instant>,
}

impl Active {
    fn remember_message(&mut self, text: String, final_answer: bool) -> Result<(), String> {
        if text.len() > MAX_ANSWER {
            return if final_answer {
                Err("Codex answer exceeded the 4 MiB limit".into())
            } else {
                // Commentary was not answer-limited before partial retention.
                // Skip this snapshot without changing native turn semantics.
                Ok(())
            };
        }
        if final_answer {
            if !text.is_empty() {
                self.partial = None;
            }
            self.answer = Some(text);
        } else if !text.is_empty() && self.answer.as_deref().is_none_or(str::is_empty) {
            // Retain a single snapshot for failed turns only. Once a nonempty
            // final answer exists it owns the text budget and takes precedence.
            self.partial = Some(text);
        }
        Ok(())
    }

    // This accounts for unanswered requests only; request metadata never selects
    // the authoritative turn ID. Keep the messages themselves for ordered replay.
    fn track_early(&mut self, message: &ServerMessage, thread_id: &str) -> Result<(), String> {
        match message {
            ServerMessage::Request(request) => {
                let key = request_key(&request.id);
                if !self.early_requests.contains_key(&key) && self.early_requests.len() >= 32 {
                    return Err("Codex exceeded the 32 pending interaction limit".into());
                }
                self.early_requests.insert(key, request.params.clone());
            }
            ServerMessage::Notification(notification)
                if notification.method == "serverRequest/resolved" =>
            {
                let params = &notification.params;
                if let Some(id) = params.get("requestId") {
                    let key = format!("codex:{id}");
                    if let Some(request) = self.early_requests.get(&key) {
                        // Without an authoritative ID, only release capacity when
                        // the resolution unambiguously applies to this request.
                        let same_thread = [params, request].iter().all(|p| {
                            p.get("threadId")
                                .and_then(Value::as_str)
                                .is_none_or(|id| id == thread_id)
                        });
                        let same_turn =
                            params
                                .get("turnId")
                                .and_then(Value::as_str)
                                .is_none_or(|id| {
                                    request.get("turnId").and_then(Value::as_str) == Some(id)
                                });
                        if same_thread && same_turn {
                            self.early_requests.remove(&key);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn matches(&self, native_id: &str) -> bool {
        self.native_id.as_deref() == Some(native_id)
    }

    fn reject_pending(&mut self, handle: &AppServerHandle) {
        for (_, id) in self.requests.drain() {
            let _ = handle.respond_error(id, -32000, "Due turn has finished");
        }
    }

    fn result(self, thread_id: &str, turn: Option<Turn>, error: Option<RunError>) -> RunResult {
        let mut result = RunResult::new(Provider::Codex);
        result.run_id = Some(self.id);
        result.session_id = Some(thread_id.to_owned());
        result.duration_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        result.usage = self.usage;
        result.text = turn
            .as_ref()
            .and_then(Turn::final_message)
            .map(str::to_owned)
            .or(self.answer)
            .unwrap_or_default();
        let unsuccessful = error.is_some()
            || turn
                .as_ref()
                .is_none_or(|turn| !matches!(turn.status, TurnStatus::Completed));
        if unsuccessful && result.text.is_empty() {
            result.text = self.partial.unwrap_or_default();
        }
        if result.text.len() > MAX_ANSWER {
            result.text.clear();
            return result.fail(RunError::protocol("Codex answer exceeded the 4 MiB limit"));
        }
        if let Some(error) = error {
            return result.fail(error);
        }
        match turn.map(|turn| (turn.status, turn.error)) {
            Some((TurnStatus::Completed, _)) => result,
            Some((TurnStatus::Interrupted, _)) => result.fail(RunError::new(
                ErrorKind::Cancelled,
                "Codex turn interrupted",
            )),
            Some((TurnStatus::Failed, error)) => result.fail(RunError::new(
                ErrorKind::Provider,
                error
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "Codex turn failed".into()),
            )),
            _ => result.fail(RunError::protocol(
                "Codex completion has no terminal status",
            )),
        }
    }
}

impl Backend {
    pub async fn open(
        options: &OpenOptions,
        sink: EventSink,
        fork_source: Option<&str>,
    ) -> Result<Self, String> {
        let turn_timeout = options.timeout_duration()?;
        let mut builder = Codex::builder()
            .timeout(CONTROL_TIMEOUT)
            .termination_grace(Duration::from_secs(5))
            .output_limit(MAX_OUTPUT);
        if let Some(binary) = &options.binary {
            builder = builder.binary(binary);
        }
        if let Some(cwd) = &options.cwd {
            builder = builder.working_dir(cwd);
        }
        for argument in &options.native_args {
            builder = builder.arg(argument);
        }
        let client = builder.build().map_err(|e| e.to_string())?;
        let server = AppServer::builder(&client)
            .client_info("due", env!("CARGO_PKG_VERSION"))
            .start()
            .await
            .map_err(|e| e.to_string())?;
        let handle = server.handle();
        let opened = if let Some(id) = fork_source.or(options.resume.as_deref()) {
            let method = if fork_source.is_some() {
                "thread/fork"
            } else {
                "thread/resume"
            };
            let mut params = json!({"threadId": id});
            if fork_source.is_some() {
                params["excludeTurns"] = json!(true);
            }
            if let Some(cwd) = &options.cwd {
                params["cwd"] = json!(cwd);
            }
            if let Some(model) = &options.model {
                params["model"] = json!(model);
            }
            handle
                .request(method, params)
                .await
                .map_err(|error| report_rpc_error(error, &sink, None))
                .and_then(|response| {
                    sink.item(
                        "native_response",
                        json!({"provider":"codex", "method":method, "response":response}),
                        None,
                    );
                    if let Some(source) = fork_source
                        && let Some(ancestry) = response
                            .pointer("/thread/forkedFromId")
                            .and_then(Value::as_str)
                        && ancestry != source
                    {
                        return Err("Codex fork reported unexpected source ancestry".into());
                    }
                    response
                        .pointer("/thread/id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| format!("Codex {method} response has no thread ID"))
                })
        } else {
            let mut params = ThreadStartParams::new().ephemeral(options.ephemeral);
            if let Some(cwd) = &options.cwd {
                params = params.cwd(cwd);
            }
            if let Some(model) = &options.model {
                params = params.model(model);
            }
            handle
                .thread_start(params)
                .await
                .map(|thread| {
                    sink.item(
                        "native_response",
                        json!({"provider":"codex", "method":"thread/start", "response":thread.raw}),
                        None,
                    );
                    thread.id
                })
                .map_err(|error| report_rpc_error(error, &sink, None))
        };
        let thread_id = match opened {
            Ok(id) => id,
            Err(error) => {
                let cleanup = server.shutdown().await;
                return Err(match cleanup {
                    Ok(()) => error,
                    Err(e) => format!("{error}; shutdown: {e}"),
                });
            }
        };
        sink.native_session(thread_id.clone());
        let (commands, receiver) = mpsc::channel(16);
        let sender = commands.clone();
        let effort = options.effort.map(|value| value.as_str().to_owned());
        let owner = tokio::spawn(own(
            server,
            handle,
            thread_id,
            effort,
            turn_timeout,
            sink,
            sender,
            receiver,
        ));
        Ok(Self {
            commands,
            owner: Some(owner),
        })
    }

    async fn call(&self, command: impl FnOnce(Reply) -> Command) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(command(reply))
            .await
            .map_err(|_| "Codex session is closed".to_owned())?;
        answer
            .await
            .map_err(|_| "Codex session closed during operation".to_owned())?
    }

    pub async fn start(&mut self, turn_id: String, prompt: String) -> Result<(), String> {
        self.call(|reply| Command::Start {
            id: turn_id,
            prompt,
            reply,
        })
        .await
    }

    pub fn enqueue_interrupt(&self, turn_id: &str) -> Result<super::InterruptAnswer, String> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .try_send(Command::Interrupt {
                id: turn_id.to_owned(),
                deadline: tokio::time::Instant::now() + CONTROL_TIMEOUT,
                reply,
            })
            .map_err(|error| format!("Cannot enqueue Codex interruption: {error}"))?;
        Ok(answer)
    }

    pub async fn reply(
        &mut self,
        request_id: &str,
        response: Value,
        expected: Option<(String, u64)>,
    ) -> Result<(), String> {
        self.call(|reply| Command::Respond {
            id: request_id.to_owned(),
            response,
            expected,
            reply,
        })
        .await
    }

    pub async fn close(mut self) -> Result<(), CloseError> {
        let _ = self.commands.send(Command::Close).await;
        self.owner
            .take()
            .expect("owner task")
            .await
            .map_err(|e| CloseError::Unconfirmed(format!("Codex owner task failed: {e}")))?
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.abort();
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn own(
    mut server: AppServer,
    handle: AppServerHandle,
    thread_id: String,
    effort: Option<String>,
    turn_timeout: Option<Duration>,
    sink: EventSink,
    sender: mpsc::Sender<Command>,
    mut commands: mpsc::Receiver<Command>,
) -> Result<(), CloseError> {
    let mut active: Option<Active> = None;
    let mut interrupt: Option<PendingInterrupt> = None;
    let failure = 'owner: loop {
        // Completion retires only unsent intent. Once dispatched, the exact
        // native/Due identities and response future live independently of Active.
        if interrupt.as_ref().is_some_and(|control| {
            control.native_id.is_none()
                && !active
                    .as_ref()
                    .is_some_and(|turn| turn.id == control.due_turn)
        }) {
            interrupt
                .take()
                .unwrap()
                .reject("Codex turn ended before interruption was dispatched");
        }
        let deadline = active.as_ref().and_then(|turn| turn.deadline);
        let control_deadline = interrupt
            .as_ref()
            .filter(|control| control.pending())
            .map(|control| control.deadline);
        tokio::select! {
            // If native acceptance and completion are both ready, consume the
            // acknowledgement first. The owner retains the future across turn changes.
            biased;
            _ = deadline_elapsed(control_deadline) => {
                if let Some(mut control) = interrupt.take() {
                    let message = if control.native_id.is_some() {
                        "Codex interruption timed out; acknowledgement unknown, request may already have been delivered"
                    } else { "Codex interruption timed out before dispatch; intent removed" };
                    control.reject(message);
                }
            },
            _ = deadline_elapsed(deadline) => break Some((ErrorKind::Timeout, "Codex turn deadline exceeded; session closed".to_owned())),
            result = interrupt_accepted(&mut interrupt) => {
                let mut control = interrupt.take().expect("pending interrupt");
                if result.is_ok() {
                    sink.item("interrupt_requested", json!({}), Some(&control.due_turn));
                }
                if let Some(reply) = control.reply.take() { let _ = reply.send(result); }
            },
            // Fairly select native traffic and commands beneath control/deadline
            // priority, so neither a chatty provider nor callers starve the other.
            event = async {
                tokio::select! {
                    message = server.next_message() => (Some(message), None),
                    command = commands.recv() => (None, Some(command)),
                }
            } => match event {
            (Some(message), _) => match message {
                Ok(Some(message @ (ServerMessage::Notification(_) | ServerMessage::Request(_)))) => {
                    let (kind, data, params) = match &message {
                        ServerMessage::Notification(n) => ("native", notification_item(n), &n.params),
                        ServerMessage::Request(r) => ("native_request", json!({"provider":"codex", "id":r.id, "method":r.method, "params":r.params}), &r.params),
                        _ => unreachable!(),
                    };
                    sink.item(kind, data.clone(), native_turn_id(params, &thread_id, active.as_ref()));
                    if let Some(turn) = active.as_mut().filter(|turn| turn.native_id.is_none()) {
                        let bytes = data.to_string().len();
                        if turn.early.len() >= 256 || turn.early_bytes.saturating_add(bytes) > 1024 * 1024 {
                            if let ServerMessage::Request(request) = message {
                                let _ = handle.respond_error(request.id, -32000, "Early-event limit exceeded");
                            }
                            break Some((ErrorKind::Protocol, "Codex start response lagged beyond the early-event limit".into()));
                        }
                        if let Err(error) = turn.track_early(&message, &thread_id) {
                            if let ServerMessage::Request(request) = message {
                                let _ = handle.respond_error(request.id, -32000, "Too many pending Due interactions");
                            }
                            break Some((ErrorKind::Protocol, error));
                        }
                        turn.early_bytes += bytes;
                        turn.early.push(message);
                    } else if let Err(error) = replay(message, &thread_id, &mut active, &sink, &handle) {
                        break Some((ErrorKind::Protocol, error));
                    }
                }
                Ok(Some(_)) => {},
                Ok(None) => break Some((ErrorKind::Protocol, "Codex app-server exited".to_owned())),
                Err(error) => break Some((ErrorKind::Protocol, error.to_string())),
            },
            (_, Some(command)) => match command {
                Some(Command::Start { id, prompt, reply }) => {
                    if active.is_some() { let _ = reply.send(Err("A Codex turn is already running".into())); continue; }
                    let now = tokio::time::Instant::now();
                    let deadline = match turn_timeout {
                        None => None,
                        Some(duration) => match now.checked_add(duration) {
                            Some(deadline) => Some(deadline),
                            None => { let _ = reply.send(Err("Turn timeout exceeds the clock range".into())); continue; }
                        }
                    };
                    active = Some(Active { id: id.clone(), native_id: None, answer: None, partial: None, usage: None,
                        requests: HashMap::new(), early: Vec::new(), early_bytes: 0, early_requests: HashMap::new(), started: Instant::now(), deadline });
                    let mut params = TurnStartParams::text(&thread_id, prompt);
                    if let Some(effort) = &effort { params = params.effort(effort); }
                    let turn_handle = handle.clone();
                    let responses = sender.clone();
                    let turn_sink = sink.clone();
                    tokio::spawn(async move {
                        // Keep the entire response; the typed helper retains only Turn.
                        let result = match serde_json::to_value(params) {
                            Ok(params) => turn_handle.request("turn/start", params).await.map_err(|error| report_rpc_error(error, &turn_sink, Some(&id))),
                            Err(error) => Err(format!("Invalid Codex turn/start parameters: {error}")),
                        };
                        let _ = responses.send(Command::Started { id, result }).await;
                    });
                    let _ = reply.send(Ok(()));
                }
                Some(Command::Started { id, result }) => {
                    if !active.as_ref().is_some_and(|turn| turn.id == id) { continue; }
                    let response = match result {
                        Ok(response) => response,
                        Err(error) => break Some((ErrorKind::Provider, error)),
                    };
                    sink.item("native_response", json!({"provider":"codex", "method":"turn/start", "response":response}), Some(&id));
                    // The completion notification may precede the start response.
                    let Some(turn) = active.as_mut().filter(|turn| turn.id == id) else { continue; };
                    match serde_json::from_value::<Turn>(response.get("turn").cloned().unwrap_or(Value::Null)) {
                        Ok(native) => {
                            turn.native_id = Some(native.id.clone());
                            let early = std::mem::take(&mut turn.early);
                            turn.early_bytes = 0;
                            turn.early_requests.clear();
                            for message in early {
                                if let Err(error) = replay(message, &thread_id, &mut active, &sink, &handle) {
                                    break 'owner Some((ErrorKind::Protocol, error));
                                }
                            }
                            if native.status != TurnStatus::InProgress
                                && let Some(mut done) = active.take() {
                                done.reject_pending(&handle);
                                sink.finished(done.result(&thread_id, Some(native), None));
                            }
                            // Replay terminal events before dispatch: intent belongs only to
                            // this turn and must never interrupt a completed or subsequent turn.
                            if let Some(turn) = active.as_mut().filter(|turn| turn.id == id)
                                && let Some(control) = interrupt.as_mut().filter(|control| control.due_turn == id) {
                                control.dispatch(handle.clone(), thread_id.clone(),
                                    turn.native_id.clone().expect("authoritative turn"), sink.clone());
                            }
                        }
                        Err(error) => break Some((ErrorKind::Protocol, format!("Codex turn/start returned no usable turn: {error}"))),
                    }
                }

                Some(Command::Interrupt { id, deadline, reply }) => {
                    if deadline <= tokio::time::Instant::now() {
                        let _ = reply.send(Err("Codex interruption expired before dispatch".into()));
                        continue;
                    }
                    if let Some(turn) = active.as_mut().filter(|turn| turn.id == id) {
                        if interrupt.is_some() {
                            let _ = reply.send(Err("A Codex interruption is already pending".into()));
                            continue;
                        }
                        interrupt = Some(PendingInterrupt::new(id, reply, deadline));
                        if let Some(native) = &turn.native_id {
                            interrupt.as_mut().unwrap().dispatch(handle.clone(), thread_id.clone(),
                                native.clone(), sink.clone());
                        } else {
                            // Observable retention, not an acknowledgement of native acceptance.
                            sink.item("interrupt_pending", json!({}), Some(&turn.id));
                        }
                    } else {
                        let _ = reply.send(Err("Unknown or finished Due turn to interrupt".into()));
                    }
                }
                Some(Command::Respond { id, response, expected, reply }) => {
                    let result = if expected.as_ref().is_some_and(|(turn, cursor)| !sink.interaction_current(turn, &id, *cursor)) {
                        Err("interaction is no longer current".into())
                    } else if !response.is_object() {
                        Err("Codex interaction response must be a JSON object".into())
                    } else if let Some(turn) = active.as_mut() {
                        if let Some(native_id) = turn.requests.get(&id) {
                            match handle.respond(native_id.clone(), response.clone()) {
                                Ok(()) => {
                                    sink.item("interaction_reply", json!({"request_id":id,"response":response}), Some(&turn.id));
                                    turn.requests.remove(&id);
                                    sink.resolved(&id);
                                    Ok(())
                                },
                                Err(error) => Err(error.to_string()),
                            }
                        } else { Err("Unknown or resolved interaction ID".into()) }
                    } else { Err("No turn is waiting for an interaction".into()) };
                    let _ = reply.send(result);
                }
                Some(Command::Close) | None => break None,
            },
            _ => unreachable!("one owner event"),
            }
        }
    };
    if let Some(mut control) = interrupt.take() {
        let reason = failure
            .as_ref()
            .map(|(_, message)| message.as_str())
            .unwrap_or("Codex session closed");
        let message = if control.native_id.is_some() {
            format!(
                "{reason}; interruption acknowledgement unknown, request may already have been delivered"
            )
        } else {
            format!("{reason}; interruption intent removed before dispatch")
        };
        control.reject(&message);
    }
    // EOF shutdown lets Codex stop commands it placed in other process groups.
    let cleanup = server.shutdown().await.map_err(|error| {
        // shutdown() reports an exit code only after wait() has reaped the child.
        if error.exit_code().is_some() {
            CloseError::AbnormalExit(error.to_string())
        } else {
            CloseError::Unconfirmed(error.to_string())
        }
    });
    if let Some(turn) = active.take() {
        let error = match &failure {
            Some((kind, message)) => RunError::new(*kind, message),
            None => RunError::new(ErrorKind::Cancelled, "Codex session closed"),
        };
        sink.finished(turn.result(&thread_id, None, Some(error)));
    }
    if let Some((_, message)) = failure {
        sink.failed(message);
    }
    if let Err(error) = &cleanup {
        sink.failed(format!("Codex shutdown failed: {error}"));
    }
    cleanup
}

async fn deadline_elapsed(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn report_rpc_error(
    error: codex_wrapper::Error,
    sink: &EventSink,
    turn_id: Option<&str>,
) -> String {
    if let codex_wrapper::Error::AppServerRpc {
        method,
        code,
        message,
        data,
    } = &error
    {
        sink.item(
            "native_response",
            json!({"provider":"codex", "method":method, "error":{"code":code,"message":message,"data":data}}),
            turn_id,
        );
    }
    error.to_string()
}

fn request_key(id: &RequestId) -> String {
    format!(
        "codex:{}",
        serde_json::to_string(id).expect("request IDs serialize")
    )
}

fn notification_item(notification: &Notification) -> Value {
    json!({"provider":"codex", "method":notification.method, "params":notification.params})
}

// Never select one identity from a contradictory native envelope.
fn notification_turn_id(params: &Value) -> Result<Option<&str>, String> {
    // Notification params are opaque in the pinned wrapper. Preserve null as
    // unspecified here; typed turn/completed decoding still requires Turn.id.
    fn identity(value: Option<&Value>) -> Result<Option<&str>, String> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(id)) => Ok(Some(id.as_str())),
            Some(_) => Err("Codex notification has malformed turn ID".to_owned()),
        }
    }
    let top = identity(params.get("turnId"))?;
    let nested = identity(params.pointer("/turn/id"))?;
    if let (Some(top), Some(nested)) = (top, nested)
        && top != nested
    {
        return Err("Codex notification has contradictory turn IDs".into());
    }
    Ok(top.or(nested))
}

fn native_turn_id<'a>(
    params: &Value,
    thread_id: &str,
    active: Option<&'a Active>,
) -> Option<&'a str> {
    if params
        .get("threadId")
        .and_then(Value::as_str)
        .is_some_and(|id| id != thread_id)
    {
        return None;
    }
    let turn = active?;
    let native_id = notification_turn_id(params).ok()??;
    (turn.native_id.as_deref() == Some(native_id)).then_some(turn.id.as_str())
}

// Both live and deferred messages use the same correlation and ordering rules.
fn replay(
    message: ServerMessage,
    thread_id: &str,
    active: &mut Option<Active>,
    sink: &EventSink,
    handle: &AppServerHandle,
) -> Result<(), String> {
    let request = match message {
        ServerMessage::Notification(notification) => {
            return observe(&notification, thread_id, active, sink, handle);
        }
        ServerMessage::Request(request) => request,
        _ => return Ok(()),
    };
    let reject = |message: &str| {
        let _ = handle.respond_error(request.id.clone(), -32000, message);
    };
    let Some(turn) = active.as_mut() else {
        reject("No active Due turn");
        return Ok(());
    };
    if request
        .params
        .get("threadId")
        .and_then(Value::as_str)
        .is_some_and(|id| id != thread_id)
    {
        reject("Request belongs to another thread");
        return Ok(());
    }
    if let Some(id) = request.params.get("turnId").and_then(Value::as_str)
        && turn
            .native_id
            .as_deref()
            .is_some_and(|expected| expected != id)
    {
        reject("Request belongs to another turn");
        return Ok(());
    }
    let key = request_key(&request.id);
    if !turn.requests.contains_key(&key) && turn.requests.len() >= 32 {
        reject("Too many pending Due interactions");
        return Err("Codex exceeded the 32 pending interaction limit".into());
    }
    turn.requests.insert(key.clone(), request.id.clone());
    sink.interaction(key, json!({"provider":"codex", "id":request.id, "method":request.method, "params":request.params}));
    Ok(())
}

fn observe(
    notification: &Notification,
    thread_id: &str,
    active: &mut Option<Active>,
    sink: &EventSink,
    handle: &AppServerHandle,
) -> Result<(), String> {
    let params = &notification.params;
    if params
        .get("threadId")
        .and_then(Value::as_str)
        .is_some_and(|id| id != thread_id)
    {
        return Ok(());
    }
    let Some(turn) = active.as_mut() else {
        return Ok(());
    };
    let native_id = notification_turn_id(params)?;
    if let Some(id) = native_id
        && !turn.matches(id)
    {
        return Ok(());
    }
    if notification.method == "serverRequest/resolved"
        && let Some(id) = params.get("requestId")
    {
        let key = format!("codex:{id}");
        if turn.requests.remove(&key).is_none() {
            return Ok(());
        }
        sink.resolved(&key);
    }
    // Publish only after correlation, including when replaying early messages.
    sink.event(notification_item(notification));
    if let Some(message) = notification.agent_message() {
        let final_answer = message.is_final_answer();
        turn.remember_message(message.text, final_answer)?;
    }
    if notification.method == "thread/tokenUsage/updated" {
        // `last` is the native per-turn counter; `total` includes earlier turns.
        if let Some(usage) = params.pointer("/tokenUsage/last") {
            turn.usage = Some(Usage {
                input_tokens: usage.get("inputTokens").and_then(Value::as_u64),
                cached_input_tokens: usage.get("cachedInputTokens").and_then(Value::as_u64),
                cache_write_input_tokens: usage
                    .get("cacheWriteInputTokens")
                    .and_then(Value::as_u64),
                output_tokens: usage.get("outputTokens").and_then(Value::as_u64),
                reasoning_output_tokens: usage.get("reasoningOutputTokens").and_then(Value::as_u64),
                total_tokens: usage.get("totalTokens").and_then(Value::as_u64),
            });
        }
    }
    if notification.method == "turn/completed" {
        let completed = notification
            .turn_completed()
            .ok_or("Malformed Codex turn/completed notification")?;
        if completed.thread_id != thread_id {
            return Ok(());
        }
        let mut turn = active.take().expect("active turn");
        turn.reject_pending(handle);
        sink.finished(turn.result(thread_id, Some(completed.turn), None));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> Active {
        Active {
            id: "due-turn".into(),
            native_id: Some("native-turn".into()),
            answer: Some("earlier answer".into()),
            partial: None,
            usage: None,
            requests: HashMap::new(),
            early: Vec::new(),
            early_bytes: 0,
            early_requests: HashMap::new(),
            started: Instant::now(),
            deadline: None,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_expiry_removes_intent_and_owner_rejects_wrong_due_id() {
        use std::os::unix::fs::PermissionsExt;
        async fn observed(sink: &EventSink, kind: &str) {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let records = sink.items.read(0, 256).unwrap();
                    if records["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|i| i["kind"] == kind)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("owner did not observe expected item");
        }
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("provider");
        std::fs::write(&binary, format!("#!/usr/bin/env bash\nexport DUE_SESSION_FIXTURE_DIR=\"$PWD\"\nexport DUE_SESSION_EARLY=0\n{}",
            include_str!("../../tests/fixtures/session-agent.sh"))).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let options: OpenOptions = serde_json::from_value(json!({"provider":"codex",
            "binary":binary,"cwd":dir.path()}))
        .unwrap();
        let sink = EventSink::new(Provider::Codex, "session".into(), None);
        let mut backend = Backend::open(&options, sink.clone(), None).await.unwrap();
        let turn = sink.begin().unwrap();
        backend
            .start(turn.clone(), "startup-hold".into())
            .await
            .unwrap();
        // Inject a short absolute deadline through the same owner command path;
        // the native turn/start is still held by the parent's file gate.
        let (reply, answer) = oneshot::channel();
        backend
            .commands
            .try_send(Command::Interrupt {
                id: turn.clone(),
                reply,
                deadline: tokio::time::Instant::now() + Duration::from_millis(100),
            })
            .unwrap();
        observed(&sink, "interrupt_pending").await;
        let error = tokio::time::timeout(Duration::from_secs(2), answer)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        std::fs::write(dir.path().join("release-start"), "release").unwrap();
        observed(&sink, "native_response").await;
        // A queued stale command must be rejected by the owner, independently
        // of Manager's snapshot validation, even with a different live turn.
        let error = backend
            .enqueue_interrupt("wrong-due-turn")
            .unwrap()
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.contains("Unknown or finished"), "{error}");
        let input = std::fs::read_to_string(dir.path().join("input.jsonl")).unwrap();
        assert!(
            !input.contains("turn/interrupt"),
            "expired or stale intent was dispatched: {input}"
        );
        backend
            .enqueue_interrupt(&turn)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        observed(&sink, "turn_finished").await;
        // Once sent, timeout means unknown acknowledgement, even if the native
        // turn completed. It must not claim the request was never delivered.
        let late = sink.begin().unwrap();
        backend
            .start(late.clone(), "late-ack".into())
            .await
            .unwrap();
        let (reply, answer) = oneshot::channel();
        backend
            .commands
            .try_send(Command::Interrupt {
                id: late,
                reply,
                deadline: tokio::time::Instant::now() + Duration::from_millis(300),
            })
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), answer)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            error.contains("acknowledgement unknown")
                && error.contains("may already have been delivered"),
            "{error}"
        );
        std::fs::write(dir.path().join("release-ack"), "release").unwrap();
        let next = sink.begin().unwrap();
        backend.start(next, "followup".into()).await.unwrap();
        backend.close().await.unwrap();
        let input = std::fs::read_to_string(dir.path().join("input.jsonl")).unwrap();
        assert_eq!(
            input
                .lines()
                .filter(|l| l.contains("turn/interrupt"))
                .count(),
            2
        );
    }

    #[test]
    fn early_request_accounting_uses_request_ids_and_matching_resolutions() {
        use codex_wrapper::app_server::ServerRequest;
        let mut turn = active();
        turn.native_id = None;
        let request = |id| {
            ServerMessage::Request(ServerRequest {
                id,
                method: "item/commandExecution/requestApproval".into(),
                params: json!({"threadId":"thread", "turnId":"candidate"}),
            })
        };
        let resolved = |params| {
            ServerMessage::Notification(Notification {
                method: "serverRequest/resolved".into(),
                params,
            })
        };
        for id in 1..=32 {
            turn.track_early(&request(RequestId::Number(id)), "thread")
                .unwrap();
        }
        // A duplicate ID does not consume another slot; string IDs stay distinct.
        turn.track_early(&request(RequestId::Number(1)), "thread")
            .unwrap();
        assert!(
            turn.track_early(&request(RequestId::String("1".into())), "thread")
                .is_err()
        );
        for params in [
            json!({"threadId":"foreign", "requestId":1}),
            json!({"threadId":"thread", "turnId":"other", "requestId":1}),
            json!({"threadId":"thread", "requestId":"1"}),
        ] {
            turn.track_early(&resolved(params), "thread").unwrap();
            assert_eq!(turn.early_requests.len(), 32);
        }
        // Native resolutions may omit a turn ID, as in the live replay path.
        turn.track_early(
            &resolved(json!({"threadId":"thread", "requestId":1})),
            "thread",
        )
        .unwrap();
        assert_eq!(turn.early_requests.len(), 31);
        turn.track_early(&request(RequestId::Number(33)), "thread")
            .unwrap();
        assert_eq!(turn.early_requests.len(), 32);
        assert!(turn.native_id.is_none());
    }

    #[test]
    fn observed_turn_ids_cannot_establish_authoritative_identity() {
        let mut turn = active();
        turn.native_id = None;
        assert!(!turn.matches("previous-turn"));
        assert!(turn.native_id.is_none());
        turn.native_id = Some("current-turn".into());
        assert!(!turn.matches("previous-turn"));
        assert!(turn.matches("current-turn"));
    }

    #[test]
    fn native_failure_and_interruption_are_not_successful_results() {
        for (status, expected) in [
            ("failed", "failed"),
            ("interrupted", "cancelled"),
            ("inProgress", "failed"),
        ] {
            let turn =
                serde_json::from_value(json!({"id":"native-turn", "status":status})).unwrap();
            let result = active().result("native-thread", Some(turn), None);
            let value = serde_json::to_value(result).unwrap();
            assert_eq!(value["status"], expected);
            assert_eq!(value["session_id"], "native-thread");
            assert_eq!(value["run_id"], "due-turn");
            assert!(!value["error"].is_null());
        }
    }

    #[test]
    fn commentary_is_a_failure_only_fallback_including_an_empty_final() {
        for empty_final in [false, true] {
            for status in ["completed", "interrupted", "failed"] {
                let mut state = active();
                state.answer = None;
                state
                    .remember_message("first snapshot".into(), false)
                    .unwrap();
                state
                    .remember_message("latest snapshot".into(), false)
                    .unwrap();
                state.remember_message(String::new(), empty_final).unwrap();
                let turn =
                    serde_json::from_value(json!({"id":"native-turn","status":status})).unwrap();
                let result = state.result("native-thread", Some(turn), None);
                assert_eq!(
                    result.text,
                    if status == "completed" {
                        ""
                    } else {
                        "latest snapshot"
                    }
                );
                assert_eq!(result.error.is_none(), status == "completed");
            }
        }
    }

    #[test]
    fn explicit_failures_retain_partial_text_without_changing_the_error() {
        for kind in [
            ErrorKind::Timeout,
            ErrorKind::Protocol,
            ErrorKind::Provider,
            ErrorKind::Cancelled,
        ] {
            let mut state = active();
            state.answer = None;
            state
                .remember_message("accepted prefix".into(), false)
                .unwrap();
            let result = state.result(
                "native-thread",
                None,
                Some(RunError::new(kind, "original failure")),
            );
            assert_eq!(result.text, "accepted prefix");
            let error = result.error.unwrap();
            assert_eq!(error.kind, kind);
            assert_eq!(error.message, "original failure");
        }
    }

    #[test]
    fn final_answers_keep_precedence_on_unsuccessful_turns() {
        for status in ["completed", "interrupted", "failed"] {
            for terminal in [false, true] {
                let mut state = active();
                state.answer = None;
                state.remember_message("partial".into(), false).unwrap();
                state
                    .remember_message("observed final".into(), true)
                    .unwrap();
                state
                    .remember_message("later commentary".into(), false)
                    .unwrap();
                assert!(state.partial.is_none());
                let items = if terminal {
                    json!([{"type":"agentMessage","phase":"final_answer","text":"terminal final"}])
                } else {
                    json!([])
                };
                let turn = serde_json::from_value(
                    json!({"id":"native-turn","status":status,"items":items}),
                )
                .unwrap();
                let result = state.result("native-thread", Some(turn), None);
                assert_eq!(
                    result.text,
                    if terminal {
                        "terminal final"
                    } else {
                        "observed final"
                    }
                );
            }
        }
    }

    #[test]
    fn partial_text_is_byte_bounded_and_releases_its_budget_for_a_final_answer() {
        let mut state = active();
        state.answer = None;
        state
            .remember_message("é".repeat(MAX_ANSWER / 2), false)
            .unwrap();
        assert_eq!(state.partial.as_ref().unwrap().len(), MAX_ANSWER);
        state
            .remember_message("é".repeat(MAX_ANSWER / 2 + 1), false)
            .unwrap();
        assert_eq!(state.partial.as_ref().unwrap().len(), MAX_ANSWER);
        state.remember_message("final".into(), true).unwrap();
        assert!(state.partial.is_none());
        assert!(
            state
                .remember_message("x".repeat(MAX_ANSWER + 1), true)
                .is_err()
        );
        assert_eq!(state.answer.as_deref(), Some("final"));
    }

    #[test]
    fn terminal_items_take_precedence_over_observed_final_message() {
        let turn =
            serde_json::from_value(json!({"id":"native-turn", "status":"completed", "items":[
                {"type":"agentMessage", "phase":"commentary", "text":"working"},
                {"type":"agentMessage", "phase":"final_answer", "text":"finished"}
            ]}))
            .unwrap();
        let result = active().result("native-thread", Some(turn), None);
        assert_eq!(result.text, "finished");
        assert!(result.error.is_none());
    }

    #[test]
    fn numeric_and_string_native_request_ids_cannot_collide() {
        assert_ne!(
            request_key(&RequestId::Number(1)),
            request_key(&RequestId::String("1".into()))
        );
    }

    #[test]
    fn native_rpc_failure_keeps_structured_error_data() {
        let sink = EventSink::new(Provider::Codex, "session".into(), None);
        let data = json!({"reason":{"unknown":[{"keep":true}, 42]}});
        let message = report_rpc_error(
            codex_wrapper::Error::AppServerRpc {
                method: "turn/start".into(),
                code: -32000,
                message: "rejected".into(),
                data: Some(data.clone()),
            },
            &sink,
            Some("due-turn"),
        );
        assert!(message.contains("rejected"));
        let history = sink.items.read(0, 1).unwrap();
        let item = &history["items"][0];
        assert_eq!(item["turn_id"], "due-turn");
        assert_eq!(item["kind"], "native_response");
        assert_eq!(item["data"]["method"], "turn/start");
        assert_eq!(
            item["data"]["error"],
            json!({"code":-32000,"message":"rejected","data":data})
        );
    }

    #[test]
    fn native_items_preserve_unknown_tool_fields() {
        let params = json!({
            "threadId":"native-thread", "turnId":"native-turn",
            "item": {
                "id":"tool-1", "type":"futureTool",
                "input":{"nested":[1, {"futureFlag":true}]},
                "output":{"content":[{"type":"futureContent", "bytes":[0, 255]}]},
                "futureMetadata":{"trace":"unchanged"}
            },
            "futureEnvelopeField":{"keep":"me"}
        });
        let item = notification_item(&Notification {
            method: "item/completed".into(),
            params: params.clone(),
        });
        assert_eq!(item["params"], params);
        assert_eq!(item["method"], "item/completed");
    }

    #[test]
    fn session_items_only_claim_an_authoritatively_matched_turn() {
        let mut turn = active();
        let mut notification = Notification {
            method: "item/completed".into(),
            params: json!({"threadId":"native-thread", "turnId":"native-turn"}),
        };
        assert_eq!(
            native_turn_id(&notification.params, "native-thread", Some(&turn)),
            Some("due-turn")
        );
        assert_eq!(
            native_turn_id(&notification.params, "native-thread", None),
            None
        );
        notification.params["turnId"] = json!("previous-turn");
        assert_eq!(
            native_turn_id(&notification.params, "native-thread", Some(&turn)),
            None
        );
        notification.params = json!({"threadId":"other-thread", "turnId":"native-turn"});
        assert_eq!(
            native_turn_id(&notification.params, "native-thread", Some(&turn)),
            None
        );
        notification.params = json!({"threadId":"native-thread", "turn":{"id":"native-turn"}});
        assert_eq!(
            native_turn_id(&notification.params, "native-thread", Some(&turn)),
            Some("due-turn")
        );
        turn.native_id = None;
        assert_eq!(
            native_turn_id(&notification.params, "native-thread", Some(&turn)),
            None
        );
        notification.params = json!({"threadId":"native-thread"});
        assert_eq!(
            native_turn_id(&notification.params, "native-thread", Some(&active())),
            None
        );
    }
}
