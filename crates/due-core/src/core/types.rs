use std::{path::PathBuf, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    result::RunResult,
    types::{Effort, Provider},
};

/// Fixed requested configuration for one logical worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerOptions {
    /// Native provider.
    pub provider: Provider,
    /// Worker directory; canonicalized and frozen at admission.
    pub cwd: PathBuf,
    /// Explicit native executable, primarily useful for fixtures.
    pub binary: Option<PathBuf>,
    /// Native model name; absence uses native configuration.
    pub model: Option<String>,
    /// Requested effort, not proof of observed effort.
    pub effort: Option<Effort>,
    /// Disable persisted native history; live follow-ups remain possible.
    pub ephemeral: bool,
    /// Audited resident native policy arguments, never transport overrides.
    pub native_args: Vec<String>,
    /// Pending requests await an explicit caller response, or fail in reject mode.
    #[serde(default)]
    pub interaction_mode: InteractionMode,
}

impl WorkerOptions {
    /// Request a worker with native defaults in the selected directory.
    pub fn new(provider: Provider, cwd: impl Into<PathBuf>) -> Self {
        Self {
            provider,
            cwd: cwd.into(),
            binary: None,
            model: None,
            effort: None,
            ephemeral: false,
            native_args: Vec::new(),
            interaction_mode: InteractionMode::Manual,
        }
    }
}

/// Handling of native interactions; this never changes native permission grants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    /// Retain pending requests for explicit responses through the core.
    #[default]
    Manual,
    /// Fail the run and close that worker on an interaction.
    Reject,
}

/// One initial assignment in a fresh native conversation.
#[derive(Debug, Clone)]
pub struct Assignment {
    /// Fixed worker configuration.
    pub worker: WorkerOptions,
    /// Self-contained task; no parent transcript is inherited.
    pub prompt: String,
    /// Absolute assignment budget, including admission/startup time.
    pub timeout: Option<Duration>,
}

impl Assignment {
    /// Construct an assignment without an execution deadline.
    pub fn new(worker: WorkerOptions, prompt: impl Into<String>) -> Self {
        Self {
            worker,
            prompt: prompt.into(),
            timeout: None,
        }
    }
}

/// Another assignment; provider/workspace/configuration cannot change here.
#[derive(Debug, Clone)]
pub struct FollowUp {
    /// Direction for the next turn.
    pub prompt: String,
    /// Absolute budget for this assignment only.
    pub timeout: Option<Duration>,
}

impl FollowUp {
    /// Construct a follow-up without a deadline.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            timeout: None,
        }
    }
}

/// Opaque reference scoped to one engine, never a PID/native ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunRef(pub(super) String);

/// Opaque logical worker identity scoped to one engine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionRef(pub(super) String);

/// A candidate for native resume, not proof the history still exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSession {
    /// Observed native conversation identity.
    pub native_id: String,
    /// Explicit requested configuration; does not capture inherited grants.
    pub worker: WorkerOptions,
}

/// Continue a live worker or import persisted native history into a new owner.
#[derive(Debug, Clone)]
pub enum SessionInput {
    /// Same engine, same fixed worker configuration.
    Live(SessionRef),
    /// New resident process, native persisted history.
    Saved(SavedSession),
}
impl From<SessionRef> for SessionInput {
    fn from(s: SessionRef) -> Self {
        Self::Live(s)
    }
}
impl From<SavedSession> for SessionInput {
    fn from(s: SavedSession) -> Self {
        Self::Saved(s)
    }
}

/// Execution state independent of the terminal result's success/failure status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Admitted; native startup or turn admission is pending.
    Starting,
    /// Native turn is active.
    Running,
    /// A native interaction is waiting for the caller's response.
    AwaitingInput,
    /// Terminal result retained independently of future assignments.
    Finished,
}

/// Non-consuming observation of one assignment.
#[derive(Debug, Clone, Serialize)]
pub struct RunSnapshot {
    /// Engine assignment reference.
    pub run: RunRef,
    /// Engine logical conversation reference.
    pub session: SessionRef,
    /// Fixed provider.
    pub provider: Provider,
    /// Current execution state.
    pub state: RunState,
    /// Resume candidate when history was requested and an ID was observed.
    pub saved_session: Option<SavedSession>,
    /// Shared immutable terminal result; None while active.
    pub result: Option<Arc<RunResult>>,
    /// Current native requests, with run/generation-scoped response tokens.
    pub interactions: Vec<Interaction>,
}

/// Exact interaction generation. Native request IDs may be reused later.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionRef {
    pub(super) run: RunRef,
    pub(super) request_id: String,
    pub(super) cursor: u64,
}

/// One pending provider-specific request. Request payloads are not permission grants.
#[derive(Debug, Clone, Serialize)]
pub struct Interaction {
    /// Token to pass to Engine::respond; valid only while this generation is pending.
    pub id: InteractionRef,
    /// Complete native request as exposed by the wrapper, including unknown fields.
    pub request: serde_json::Value,
}

/// Confirmation of wrapper acceptance for response delivery, not tool completion.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseReceipt {
    /// Request generation to which the response was submitted.
    pub interaction: InteractionRef,
}

/// Completion condition for a set of engine runs.
#[derive(Debug, Clone, Copy)]
pub enum WaitUntil {
    /// At least one terminal run.
    Any,
    /// Every run terminal.
    All,
}

/// Caller waiting budget; never an execution/cancellation deadline.
#[derive(Debug, Clone, Copy)]
pub struct WaitOptions {
    /// Completion condition.
    pub until: WaitUntil,
    /// None waits indefinitely; zero is a nonblocking observation.
    pub timeout: Option<Duration>,
}
impl Default for WaitOptions {
    fn default() -> Self {
        Self {
            until: WaitUntil::All,
            timeout: None,
        }
    }
}

/// Requested snapshots, preserving input order even on a wait timeout.
#[derive(Debug, Clone, Serialize)]
pub struct WaitResult {
    /// Whether the requested completion condition was met.
    pub condition_met: bool,
    /// Non-consuming snapshots in request order.
    pub runs: Vec<RunSnapshot>,
}

/// Fixed per-engine resource limits; no automatic eviction or retries.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Total admitted runs retained until this engine is released.
    pub max_runs: usize,
    /// Total logical sessions owned by the engine.
    pub max_sessions: usize,
    /// Bound for interrupt settlement and explicit shutdown waiting.
    pub settlement_timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_runs: 64,
            max_sessions: 16,
            settlement_timeout: Duration::from_secs(90),
        }
    }
}

/// Operation error, distinct from an admitted run's unsuccessful result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// Machine-readable operation failure.
    pub kind: ErrorKind,
    /// Actionable bounded context.
    pub message: String,
}

/// Core operation error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Invalid parameters rejected before execution.
    InvalidInput,
    /// Run does not belong to this engine.
    UnknownRun,
    /// Session does not belong to this engine or is unusable.
    UnknownSession,
    /// A turn is already reserved for this session.
    Busy,
    /// The owner is shutting down or closed.
    Closed,
    /// An explicit retention/admission limit was reached.
    Capacity,
    /// A required Tokio runtime was absent.
    Runtime,
    /// Process/operation settlement could not be confirmed within its bound.
    Settlement,
    /// The request was resolved, replaced, belongs to another run or is finished.
    UnknownInteraction,
    /// Provider-specific response validation or delivery failed; never auto-retry.
    ResponseRejected,
}
impl Error {
    pub(super) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}
