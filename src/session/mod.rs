//! Experimental resident sessions. The existing one-shot provider adapters stay independent.

mod claude;
mod codex;
mod items;

use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, Notify};

pub(crate) use items::{PushQueue, SessionItems};

use crate::{
    cli::{Effort, Provider},
    result::{RunError, RunResult},
};

const MAX_EVENTS: usize = 256;
const MAX_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "mcp", derive(tower_mcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "tower_mcp::schemars"))]
#[serde(deny_unknown_fields)]
pub struct OpenOptions {
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub cwd: Option<PathBuf>,
    /// Override the native executable, primarily for local testing.
    pub binary: Option<PathBuf>,
    /// Native provider session/thread ID, not a Due session handle.
    pub resume: Option<String>,
    #[serde(default)]
    pub ephemeral: bool,
    /// Queue session items for the selected transport to stream.
    #[serde(default)]
    pub stream: bool,
    /// Per-turn deadline, such as "5m". Omitted means no deadline.
    pub timeout: Option<String>,
}

impl OpenOptions {
    pub fn timeout_duration(&self) -> Result<Option<Duration>, String> {
        self.timeout
            .as_deref()
            .map(crate::cli::parse_timeout)
            .transpose()
    }

    fn validate(&mut self) -> Result<(), String> {
        self.timeout_duration()?;
        for (name, value) in [("model", &self.model), ("resume", &self.resume)] {
            if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
                return Err(format!("{name} must not be empty"));
            }
        }
        if self.resume.is_some() && self.ephemeral {
            return Err("resume conflicts with ephemeral".into());
        }
        let cwd = self
            .cwd
            .clone()
            .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
        let cwd = cwd
            .canonicalize()
            .map_err(|e| format!("invalid cwd: {e}"))?;
        if !cwd.is_dir() {
            return Err("cwd must be a directory".into());
        }
        self.cwd = Some(cwd);
        Ok(())
    }
}

struct Turn {
    id: String,
    started: Instant,
    result: Option<Value>,
    events: VecDeque<(u64, Value, usize)>,
    bytes: usize,
    cursor: u64,
    dropped: u64,
    last_dropped: u64,
    interactions: BTreeMap<String, Value>,
}

struct State {
    provider: Provider,
    native_session_id: Option<String>,
    failure: Option<String>,
    turn: Option<Turn>,
}

#[derive(Clone)]
pub struct EventSink {
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    items: Arc<SessionItems>,
}

impl EventSink {
    fn new(provider: Provider, id: String, push: Option<Arc<PushQueue>>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                provider,
                native_session_id: None,
                failure: None,
                turn: None,
            })),
            changed: Arc::new(Notify::new()),
            items: Arc::new(SessionItems::new(id, push)),
        }
    }

    /// Preserve a provider payload independently of the latest-turn event window.
    pub fn item(&self, kind: &str, data: Value, turn_id: Option<&str>) {
        self.items.record(kind, data, turn_id);
    }

    pub fn event(&self, value: Value) {
        let mut state = self.state.lock().unwrap();
        let Some(turn) = state.turn.as_mut() else {
            return;
        };
        if turn.result.is_some() {
            return;
        }
        turn.cursor += 1;
        let bytes = value.to_string().len();
        if bytes > MAX_EVENT_BYTES {
            turn.dropped += 1;
            turn.last_dropped = turn.cursor;
        } else {
            while turn.events.len() >= MAX_EVENTS || turn.bytes + bytes > MAX_EVENT_BYTES {
                if let Some((cursor, _, bytes)) = turn.events.pop_front() {
                    turn.bytes -= bytes;
                    turn.dropped += 1;
                    turn.last_dropped = turn.last_dropped.max(cursor);
                }
            }
            turn.bytes += bytes;
            turn.events.push_back((turn.cursor, value, bytes));
        }
        drop(state);
        self.changed.notify_waiters();
    }

    pub fn native_session(&self, id: String) {
        self.state.lock().unwrap().native_session_id = Some(id);
        self.changed.notify_waiters();
    }

    pub fn interaction(&self, request_id: String, request: Value) {
        let mut state = self.state.lock().unwrap();
        if let Some(turn) = state.turn.as_mut().filter(|t| t.result.is_none()) {
            self.item(
                "interaction",
                json!({"request_id":request_id,"request":request}),
                Some(&turn.id),
            );
            turn.interactions.insert(request_id, request);
        }
        drop(state);
        self.changed.notify_waiters();
    }

    pub fn resolved(&self, request_id: &str) {
        if let Some(turn) = self.state.lock().unwrap().turn.as_mut()
            && turn.interactions.remove(request_id).is_some()
        {
            self.item(
                "interaction_resolved",
                json!({"request_id":request_id}),
                Some(&turn.id),
            );
        }
        self.changed.notify_waiters();
    }

    pub fn finished(&self, mut result: RunResult) {
        let mut state = self.state.lock().unwrap();
        if let (Some(id), Some(turn)) = (&result.run_id, &state.turn)
            && id != &turn.id
        {
            return;
        }
        if let Some(id) = &result.session_id {
            state.native_session_id = Some(id.clone());
        } else {
            result.session_id = state.native_session_id.clone();
        }
        if let Some(turn) = state.turn.as_mut().filter(|t| t.result.is_none()) {
            result.run_id = Some(turn.id.clone());
            result.duration_ms = turn
                .started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            turn.result = Some(serde_json::to_value(result).expect("RunResult is serializable"));
            turn.interactions.clear();
            self.item(
                "turn_finished",
                turn.result.clone().unwrap(),
                Some(&turn.id),
            );
        }
        drop(state);
        self.changed.notify_waiters();
    }

    pub fn failed(&self, message: String) {
        let provider = {
            let mut state = self.state.lock().unwrap();
            if state.failure.is_none() {
                self.item("session_failed", json!({"message":message}), None);
            }
            state.failure = Some(message.clone());
            state.provider
        };
        self.finished(RunResult::new(provider).fail(RunError::protocol(message)));
        self.changed.notify_waiters();
    }

    fn begin(&self) -> Result<String, String> {
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.failure {
            return Err(format!("session is unavailable: {error}"));
        }
        if state.turn.as_ref().is_some_and(|t| t.result.is_none()) {
            return Err("a turn is already running; read or interrupt it first".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        state.turn = Some(Turn {
            id: id.clone(),
            started: Instant::now(),
            result: None,
            events: VecDeque::new(),
            bytes: 0,
            cursor: 0,
            dropped: 0,
            last_dropped: 0,
            interactions: BTreeMap::new(),
        });
        Ok(id)
    }

    fn snapshot(&self, turn_id: &str, after: u64) -> Result<Value, String> {
        let state = self.state.lock().unwrap();
        let turn = state
            .turn
            .as_ref()
            .filter(|t| t.id == turn_id)
            .ok_or("unknown turn; only the latest turn is retained")?;
        if after > turn.cursor {
            return Err("after exceeds this turn's event cursor".into());
        }
        Ok(json!({
            "turn_id": turn.id,
            "native_session_id": state.native_session_id,
            "state": if turn.result.is_some() { "finished" } else if !turn.interactions.is_empty() { "awaiting_input" } else { "running" },
            "cursor": turn.cursor,
            "events": turn.events.iter().filter(|(seq, _, _)| *seq > after)
                .map(|(seq, event, _)| json!({"cursor":seq,"event":event})).collect::<Vec<_>>(),
            "events_lost": after < turn.last_dropped,
            "dropped_events": turn.dropped,
            "interactions": turn.interactions.iter().map(|(id, request)| json!({"request_id":id,"request":request})).collect::<Vec<_>>(),
            "result": turn.result,
            "session_error": state.failure,
        }))
    }
}

enum Backend {
    Claude(claude::Backend),
    Codex(codex::Backend),
}

impl Backend {
    async fn open(options: &OpenOptions, sink: EventSink) -> Result<Self, String> {
        match options.provider {
            Provider::Claude => claude::Backend::open(options, sink).await.map(Self::Claude),
            Provider::Codex => codex::Backend::open(options, sink).await.map(Self::Codex),
        }
    }
    async fn start(&mut self, id: String, prompt: String) -> Result<(), String> {
        match self {
            Self::Claude(b) => b.start(id, prompt).await,
            Self::Codex(b) => b.start(id, prompt).await,
        }
    }
    async fn interrupt(&mut self) -> Result<(), String> {
        match self {
            Self::Claude(b) => b.interrupt().await,
            Self::Codex(b) => b.interrupt().await,
        }
    }
    async fn reply(&mut self, id: &str, value: Value) -> Result<(), String> {
        match self {
            Self::Claude(b) => b.reply(id, value).await,
            Self::Codex(b) => b.reply(id, value).await,
        }
    }
    async fn close(self) -> Result<(), String> {
        match self {
            Self::Claude(b) => b.close().await,
            Self::Codex(b) => b.close().await,
        }
    }
}

struct Session {
    id: String,
    sink: EventSink,
    backend: Backend,
}

#[derive(Default)]
pub(crate) struct Manager {
    session: AsyncMutex<Option<Session>>,
    stopping: AtomicBool,
    pub(crate) push: Arc<PushQueue>,
}

impl Manager {
    pub(crate) async fn open(&self, mut options: OpenOptions) -> Result<Value, String> {
        options.validate()?;
        let mut slot = self.session.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err("server is shutting down".into());
        }
        if slot.is_some() {
            return Err("close the current session before opening another".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let sink = EventSink::new(
            options.provider,
            id.clone(),
            options.stream.then(|| self.push.clone()),
        );
        let backend = match Backend::open(&options, sink.clone()).await {
            Ok(backend) => backend,
            Err(error) => {
                sink.failed(error.clone());
                return Err(error);
            }
        };
        let response = json!({
            "session_id": id, "provider": options.provider,
            "native_session_id": sink.state.lock().unwrap().native_session_id,
            "capabilities": {"multi_turn":true,"interrupt":true,"interaction_reply":true,"steer":false,"session_read":true,"session_stream":options.stream},
            "stream": {"enabled":options.stream},
        });
        sink.item("session_opened", response.clone(), None);
        *slot = Some(Session { id, sink, backend });
        Ok(response)
    }

    pub(crate) async fn start(&self, session_id: &str, prompt: String) -> Result<Value, String> {
        if prompt.trim().is_empty() {
            return Err("prompt must not be empty".into());
        }
        let mut slot = self.session.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err("server is shutting down".into());
        }
        let session = Self::session(&mut slot, session_id)?;
        let id = session.sink.begin()?;
        session
            .sink
            .item("turn_started", json!({"prompt":prompt}), Some(&id));
        if let Err(error) = session.backend.start(id.clone(), prompt).await {
            session.sink.failed(error.clone());
            return Err(error);
        }
        Ok(json!({"turn_id":id,"session_id":session_id}))
    }

    pub(crate) async fn read(
        &self,
        session_id: &str,
        turn_id: &str,
        after: u64,
        wait_ms: u64,
    ) -> Result<Value, String> {
        if wait_ms > 10_000 {
            return Err("wait_ms must be at most 10000".into());
        }
        let sink = {
            let mut slot = self.session.lock().await;
            Self::session(&mut slot, session_id)?.sink.clone()
        };
        let notified = sink.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let snapshot = sink.snapshot(turn_id, after)?;
        if wait_ms != 0 && snapshot["state"] == "running" && snapshot["cursor"] == after {
            let _ = tokio::time::timeout(Duration::from_millis(wait_ms), notified).await;
            sink.snapshot(turn_id, after)
        } else {
            Ok(snapshot)
        }
    }

    pub(crate) async fn session_read(
        &self,
        session_id: &str,
        after: u64,
        limit: usize,
    ) -> Result<Value, String> {
        let sink = {
            let mut slot = self.session.lock().await;
            Self::session(&mut slot, session_id)?.sink.clone()
        };
        sink.items.read(after, limit)
    }

    pub(crate) async fn interrupt(&self, session_id: &str, turn_id: &str) -> Result<Value, String> {
        let mut slot = self.session.lock().await;
        let session = Self::session(&mut slot, session_id)?;
        let snapshot = session.sink.snapshot(turn_id, 0)?;
        if snapshot["state"] == "finished" {
            return Err("turn is already finished".into());
        }
        session.backend.interrupt().await?;
        Ok(json!({"interrupt_requested":true,"turn_id":turn_id}))
    }

    pub(crate) async fn reply(
        &self,
        session_id: &str,
        turn_id: &str,
        request_id: &str,
        response: Value,
    ) -> Result<Value, String> {
        let mut slot = self.session.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err("server is shutting down".into());
        }
        let session = Self::session(&mut slot, session_id)?;
        {
            let state = session.sink.state.lock().unwrap();
            let turn = state
                .turn
                .as_ref()
                .filter(|t| t.id == turn_id && t.result.is_none())
                .ok_or("unknown or finished turn")?;
            if !turn.interactions.contains_key(request_id) {
                return Err("unknown or resolved interaction".into());
            }
        }
        session.backend.reply(request_id, response).await?;
        Ok(json!({"replied":true,"request_id":request_id}))
    }

    pub(crate) async fn close(&self, session_id: Option<&str>) -> Result<Value, String> {
        let mut slot = self.session.lock().await;
        if let Some(id) = session_id {
            Self::session(&mut slot, id)?;
        }
        let Some(session) = slot.take() else {
            return Ok(json!({"closed":true}));
        };
        let outcome = session.backend.close().await;
        if let Err(error) = &outcome {
            session.sink.failed(error.clone());
        } else {
            session.sink.state.lock().unwrap().failure = Some("session closed".into());
            let provider = session.sink.state.lock().unwrap().provider;
            session
                .sink
                .finished(RunResult::new(provider).fail(RunError::protocol("session closed")));
        }
        session.sink.item(
            "session_closed",
            json!({"error":outcome.as_ref().err()}),
            None,
        );
        outcome?;
        Ok(
            json!({"closed":true,"native_session_id":session.sink.state.lock().unwrap().native_session_id}),
        )
    }

    pub(crate) async fn shutdown(&self) -> Result<Value, String> {
        // EOF must reject queued opens before waiting for any current operation.
        self.stopping.store(true, Ordering::Release);
        self.close(None).await
    }

    fn session<'a>(slot: &'a mut Option<Session>, id: &str) -> Result<&'a mut Session, String> {
        slot.as_mut()
            .filter(|s| s.id == id)
            .ok_or_else(|| "unknown session handle".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_rejects_queued_session_creation() {
        let manager = Manager::default();
        manager.shutdown().await.unwrap();
        let options: OpenOptions =
            serde_json::from_value(json!({"provider":"claude", "binary":"/no-provider-needed"}))
                .unwrap();
        assert_eq!(
            manager.open(options).await.unwrap_err(),
            "server is shutting down"
        );
    }

    #[test]
    fn bounded_events_report_loss_and_keep_terminal_result() {
        let sink = EventSink::new(Provider::Claude, "test".into(), None);
        let id = sink.begin().unwrap();
        for n in 0..300 {
            sink.event(json!({"n":n}));
        }
        let snapshot = sink.snapshot(&id, 0).unwrap();
        assert_eq!(snapshot["events"].as_array().unwrap().len(), MAX_EVENTS);
        assert_eq!(snapshot["dropped_events"], 44);
        assert_eq!(snapshot["events_lost"], true);
        sink.native_session("native".into());
        sink.finished(RunResult::new(Provider::Claude));
        let snapshot = sink.snapshot(&id, 300).unwrap();
        assert_eq!(snapshot["state"], "finished");
        assert_eq!(snapshot["result"]["session_id"], "native");
    }

    #[test]
    fn one_turn_at_a_time_and_stale_turns_are_rejected() {
        let sink = EventSink::new(Provider::Codex, "test".into(), None);
        let first = sink.begin().unwrap();
        assert!(sink.begin().is_err());
        sink.interaction("ask".into(), json!({"method":"approve"}));
        assert_eq!(sink.snapshot(&first, 0).unwrap()["state"], "awaiting_input");
        sink.finished(RunResult::new(Provider::Codex));
        assert!(
            sink.snapshot(&first, 0).unwrap()["interactions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let second = sink.begin().unwrap();
        assert!(sink.snapshot(&first, 0).is_err());
        assert!(sink.snapshot(&second, 1).is_err());
    }

    #[test]
    fn oversized_event_gaps_and_stale_results_are_explicit() {
        let sink = EventSink::new(Provider::Claude, "test".into(), None);
        let id = sink.begin().unwrap();
        sink.event(json!({"small":1}));
        sink.event(json!({"large":"x".repeat(MAX_EVENT_BYTES)}));
        sink.event(json!({"small":2}));
        assert_eq!(sink.snapshot(&id, 0).unwrap()["events_lost"], true);
        assert_eq!(sink.snapshot(&id, 2).unwrap()["events_lost"], false);
        let mut stale = RunResult::new(Provider::Claude);
        stale.run_id = Some("previous-turn".into());
        sink.finished(stale);
        assert_eq!(sink.snapshot(&id, 3).unwrap()["state"], "running");
    }
}
