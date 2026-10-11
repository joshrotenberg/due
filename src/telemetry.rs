use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::{Map, Value, json};
use tracing::{
    Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
    util::SubscriberInitExt,
};

use crate::{
    cli::{Provider, RunArgs, TraceFormat},
    result::{RunError, RunResult, TraceSummary},
};

const TARGET: &str = "due::progress";
const QUEUE_CAPACITY: usize = 1024;

/// Categories describe observed native activity, not equal units of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Command,
    FileChange,
    McpCall,
    WebSearch,
    PlanUpdate,
    StatusUpdate,
    ToolCall,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

/// The sole source of trace records. Native payloads never enter this API.
#[derive(Clone, Default)]
pub struct Progress(Option<Arc<Run>>);

struct Run {
    id: String,
    provider: Provider,
    start: Instant,
    span: tracing::Span,
    state: Mutex<State>,
    dropped: Arc<AtomicU64>,
}

struct State {
    seq: u64,
    last_activity: Instant,
    session: Option<String>,
    pid: Option<u32>,
    pgid: Option<u32>,
    activities_started: u64,
    activities_completed: u64,
    activities_failed: u64,
    messages: u64,
    active_activities: HashMap<String, Instant>,
}

enum Message {
    Record(String),
    Shutdown,
}

pub struct TraceGuard {
    sender: mpsc::SyncSender<Message>,
    done: mpsc::Receiver<()>,
    failed: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}

pub fn start(run: &RunArgs) -> Result<(Progress, Option<TraceGuard>), RunError> {
    if run.trace.is_none() && run.trace_file.is_none() {
        if run.trace_interval.is_some() {
            return Err(RunError::invalid(
                "--trace-interval requires --trace or --trace-file",
            ));
        }
        return Ok((Progress::default(), None));
    }
    let file = run
        .trace_file
        .as_ref()
        .map(|path| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(path)
                .map_err(|error| RunError::io("create trace file", error))
        })
        .transpose()?;
    let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (done_sender, done) = mpsc::channel();
    let failed = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicU64::new(0));
    let worker_failed = Arc::clone(&failed);
    let stderr_format = run.trace;
    thread::Builder::new()
        .name("due-trace".into())
        .spawn(move || {
            write_records(receiver, file, stderr_format, &worker_failed);
            let _ = done_sender.send(());
        })
        .map_err(|error| RunError::io("start trace writer", error))?;
    let layer = ProgressLayer {
        sender: sender.clone(),
        dropped: Arc::clone(&dropped),
    };
    // A hard target allowlist is deliberate. Wrapper debug spans/events may
    // contain full argv, diagnostics, or raw messages; RUST_LOG must not expose them.
    tracing_subscriber::registry()
        .with(
            layer.with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.target() == TARGET
            })),
        )
        .try_init()
        .map_err(|_| {
            RunError::new(
                crate::result::ErrorKind::Io,
                "could not initialize progress tracing",
            )
        })?;
    let now = Instant::now();
    let id = uuid::Uuid::new_v4().to_string();
    let span =
        tracing::info_span!(target: TARGET, "run", run_id = %id, provider = run.provider.as_str());
    let progress = Progress(Some(Arc::new(Run {
        id,
        provider: run.provider,
        start: now,
        span,
        state: Mutex::new(State {
            seq: 0,
            last_activity: now,
            session: None,
            pid: None,
            pgid: None,
            activities_started: 0,
            activities_completed: 0,
            activities_failed: 0,
            messages: 0,
            active_activities: HashMap::new(),
        }),
        dropped: Arc::clone(&dropped),
    })));
    Ok((
        progress,
        Some(TraceGuard {
            sender,
            done,
            failed,
            dropped,
        }),
    ))
}

impl Progress {
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }

    pub fn run_id(&self) -> Option<&str> {
        self.0.as_ref().map(|run| run.id.as_str())
    }

    pub fn started(&self, args: &RunArgs) {
        self.emit("run_started", true, |_, fields| {
            fields.insert("resumed".into(), json!(args.resume.is_some()));
            fields.insert("ephemeral".into(), json!(args.ephemeral));
            fields.insert("dry_run".into(), json!(args.dry_run));
            if let Some(timeout) = args.timeout {
                fields.insert("timeout_ms".into(), json!(millis(timeout)));
            }
        });
    }

    pub fn prompt_ready(&self, bytes: usize) {
        self.emit("prompt_ready", true, |_, fields| {
            fields.insert("prompt_bytes".into(), json!(bytes));
        });
    }

    pub fn spawned(&self, pid: u32, pgid: Option<u32>) {
        self.emit("process_started", true, |state, _| {
            state.pid = Some(pid);
            state.pgid = pgid;
        });
    }

    pub fn session(&self, id: &str) {
        let Some(run) = &self.0 else { return };
        let id = identifier(id);
        let mut state = run.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.session.as_ref() == Some(&id) {
            return;
        }
        state.session = Some(id);
        self.emit_locked(run, &mut state, "session_started", true, Map::new());
    }

    pub fn phase(&self, event: &'static str) {
        self.emit(event, true, |_, _| ());
    }

    pub fn activity_started(&self, id: Option<&str>, kind: ActivityKind, tool: Option<&str>) {
        self.emit("activity_started", true, |state, fields| {
            activity_fields(fields, id, kind, tool);
            state.activities_started += 1;
            if let Some(id) = id
                && state.active_activities.len() < 1024
            {
                state
                    .active_activities
                    .insert(identifier(id), Instant::now());
            }
        });
    }

    pub fn activity_completed(
        &self,
        id: Option<&str>,
        kind: ActivityKind,
        status: ActivityStatus,
        tool: Option<&str>,
    ) {
        self.emit("activity_completed", true, |state, fields| {
            activity_fields(fields, id, kind, tool);
            fields.insert("status".into(), json!(status));
            state.activities_completed += 1;
            if status == ActivityStatus::Failed {
                state.activities_failed += 1;
            }
            if let Some(start) = id.and_then(|id| state.active_activities.remove(&identifier(id))) {
                fields.insert(
                    "activity_duration_ms".into(),
                    json!(millis(start.elapsed())),
                );
            }
        });
    }

    pub fn assistant(&self, bytes: usize) {
        self.emit("message_completed", true, |state, fields| {
            state.messages += 1;
            fields.insert("message_bytes".into(), json!(bytes));
        });
    }

    pub fn heartbeat(&self) {
        self.emit("heartbeat", false, |_, fields| {
            fields.insert("state".into(), json!("waiting"));
        });
    }

    pub fn finished(&self, result: &RunResult) {
        self.emit("run_finished", true, |_, fields| {
            fields.insert("scope".into(), json!("execution"));
            fields.insert("status".into(), json!(result.status));
            fields.insert("exit_code".into(), json!(result.exit_code()));
            fields.insert("duration_ms".into(), json!(result.duration_ms));
            if let Some(error) = &result.error {
                fields.insert("error_kind".into(), json!(error.kind));
                fields.insert("provider_exit_code".into(), json!(error.provider_exit_code));
            }
            if let Some(usage) = &result.usage {
                fields.insert("usage".into(), json!(usage));
            }
            if let Some(cost) = result.cost_usd {
                fields.insert("cost_usd".into(), json!(cost));
            }
        });
    }

    fn emit(
        &self,
        event: &'static str,
        active: bool,
        change: impl FnOnce(&mut State, &mut Map<String, Value>),
    ) {
        let Some(run) = &self.0 else { return };
        let mut state = run.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut fields = Map::new();
        change(&mut state, &mut fields);
        self.emit_locked(run, &mut state, event, active, fields);
    }

    fn emit_locked(
        &self,
        run: &Run,
        state: &mut State,
        event: &'static str,
        active: bool,
        mut fields: Map<String, Value>,
    ) {
        let now = Instant::now();
        state.seq += 1;
        fields.insert("schema_version".into(), json!(1));
        fields.insert("run_id".into(), json!(run.id));
        fields.insert("provider".into(), json!(run.provider));
        fields.insert("seq".into(), json!(state.seq));
        fields.insert(
            "elapsed_ms".into(),
            json!(millis(now.duration_since(run.start))),
        );
        fields.insert(
            "timestamp_ms".into(),
            json!(millis(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
            )),
        );
        fields.insert("event".into(), json!(event));
        fields.insert(
            "idle_ms".into(),
            json!(millis(now.duration_since(state.last_activity))),
        );
        fields.insert("activities_started".into(), json!(state.activities_started));
        fields.insert(
            "activities_completed".into(),
            json!(state.activities_completed),
        );
        fields.insert("activities_failed".into(), json!(state.activities_failed));
        fields.insert("messages".into(), json!(state.messages));
        fields.insert(
            "dropped_records".into(),
            json!(run.dropped.load(Ordering::Relaxed)),
        );
        if let Some(session) = &state.session {
            fields.insert("session_id".into(), json!(session));
        }
        if let Some(pid) = state.pid {
            fields.insert("pid".into(), json!(pid));
        }
        if let Some(pgid) = state.pgid {
            fields.insert("pgid".into(), json!(pgid));
        }
        if active {
            state.last_activity = now;
        }
        let record = Value::Object(fields).to_string();
        run.span
            .in_scope(|| tracing::info!(target: TARGET, record = record.as_str()));
    }
}

fn activity_fields(
    fields: &mut Map<String, Value>,
    id: Option<&str>,
    kind: ActivityKind,
    tool: Option<&str>,
) {
    fields.insert("activity".into(), json!(kind));
    if let Some(id) = id {
        fields.insert("activity_id".into(), json!(identifier(id)));
    }
    if let Some(tool) = tool {
        fields.insert("tool".into(), json!(identifier(tool)));
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn identifier(value: &str) -> String {
    if value.is_empty()
        || value.len() > 160
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.:/".contains(c))
    {
        "redacted".into()
    } else {
        value.into()
    }
}

struct ProgressLayer {
    sender: mpsc::SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}

impl<S: Subscriber> Layer<S> for ProgressLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut record = RecordVisitor(None);
        event.record(&mut record);
        if let Some(record) = record.0
            && self.sender.try_send(Message::Record(record)).is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct RecordVisitor(Option<String>);
impl Visit for RecordVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "record" {
            self.0 = Some(value.into());
        }
    }
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

fn write_records(
    receiver: mpsc::Receiver<Message>,
    mut file: Option<File>,
    mut stderr_format: Option<TraceFormat>,
    failed: &AtomicBool,
) {
    for message in receiver {
        let Message::Record(record) = message else {
            break;
        };
        let line = format!("{record}\n");
        if let Some(writer) = &mut file
            && writer
                .write_all(line.as_bytes())
                .and_then(|_| writer.flush())
                .is_err()
        {
            file = None;
            failed.store(true, Ordering::Relaxed);
        }
        if let Some(format) = stderr_format {
            let text = if format == TraceFormat::Json {
                line
            } else {
                human_record(&record)
            };
            if io::stderr().write_all(text.as_bytes()).is_err() {
                stderr_format = None;
                failed.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn human_record(record: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(record) else {
        return String::new();
    };
    let elapsed = value["elapsed_ms"].as_u64().unwrap_or(0) as f64 / 1000.0;
    let provider = value["provider"].as_str().unwrap_or("due");
    let id = value["run_id"].as_str().unwrap_or("");
    let mut line = format!(
        "[{elapsed:8.3}s] {provider}/{} {}",
        &id[..id.len().min(8)],
        value["event"].as_str().unwrap_or("progress")
    );
    for field in [
        "pid",
        "session_id",
        "activity_id",
        "activity",
        "tool",
        "activity_duration_ms",
        "state",
        "idle_ms",
        "status",
        "exit_code",
        "error_kind",
    ] {
        if let Some(item) = value.get(field) {
            line.push_str(&format!(
                " {field}={}",
                item.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| item.to_string())
            ));
        }
    }
    line.push('\n');
    line
}

impl TraceGuard {
    /// Drain normally, but never let a blocked trace consumer hold execution open indefinitely.
    pub fn finish(self) -> TraceSummary {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut shutdown_sent = false;
        while Instant::now() < deadline {
            match self.sender.try_send(Message::Shutdown) {
                Ok(()) => {
                    shutdown_sent = true;
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => break,
                Err(mpsc::TrySendError::Full(_)) => thread::sleep(Duration::from_millis(5)),
            }
        }
        let drained = shutdown_sent
            && self
                .done
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .is_ok();
        let dropped_records = self.dropped.load(Ordering::Relaxed);
        // Never touch stderr here: the writer may hold its lock while blocked.
        // Report sink health in the final JSON result without changing its status.
        TraceSummary {
            incomplete: !drained || self.failed.load(Ordering::Relaxed) || dropped_records > 0,
            dropped_records,
        }
    }
}
