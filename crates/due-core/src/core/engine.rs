use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    runtime::Handle,
    sync::{Mutex as AsyncMutex, Notify, watch},
    time::{Instant as TokioInstant, timeout},
};

use super::*;
use crate::{
    result::{ErrorKind as RunErrorKind, RunError, RunResult, Status},
    session::{Manager, OpenOptions, SessionItems},
};

// The wrapper open operation includes pipe creation and native initialization.
// Serialize cooperating Darwin opens across engines through that boundary.
// Active turns do not hold this guard. Never use a blocking lock across await.
#[cfg(target_os = "macos")]
static OPEN_NATIVE: AsyncMutex<()> = AsyncMutex::const_new(());
const MAX_INPUT: usize = 1024 * 1024;

enum SessionStart {
    Fresh,
    Resume(String),
    Fork(String),
}

struct Worker {
    fork_source: Option<String>,
    options: WorkerOptions,
    manager: Manager,
    handle: Mutex<Option<String>>,
    native: Mutex<Option<String>>,
    unavailable: AtomicBool,
    cleanup: AsyncMutex<Option<Result<(), String>>>,
    items: Arc<SessionItems>,
}
struct Session {
    worker: Arc<Worker>,
    active: Option<RunRef>,
    releasing: bool,
}
struct Run {
    snapshot: RunSnapshot,
    cancel: watch::Sender<bool>,
    responses: HashSet<InteractionRef>,
    accepting_responses: bool,
    response_error: Option<String>,
}
#[derive(Default)]
struct State {
    closing: bool,
    runs: HashMap<RunRef, Run>,
    sessions: HashMap<SessionRef, Session>,
}
struct Inner {
    limits: Limits,
    runtime: Handle,
    state: Mutex<State>,
    changed: Notify,
    shutdown: AsyncMutex<()>,
}

/// One owner of bounded native workers and immutable per-assignment results.
///
/// Calls require a running Tokio runtime. Sharing `&Engine` (or `Arc<Engine>`)
/// permits concurrent operations. Shutdown denies new work and settles every
/// owned worker. Drop requests best-effort cleanup; explicit shutdown reports it.
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Create an engine on the current Tokio runtime with explicit resource limits.
    pub fn new(limits: Limits) -> Result<Self, Error> {
        if limits.max_runs == 0 || limits.max_sessions == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "engine limits must be nonzero",
            ));
        }
        validate_duration(Some(limits.settlement_timeout))?;
        let runtime = Handle::try_current().map_err(|_| {
            Error::new(
                ErrorKind::Runtime,
                "Engine requires a running Tokio runtime",
            )
        })?;
        Ok(Self {
            inner: Arc::new(Inner {
                limits,
                runtime,
                state: Mutex::default(),
                changed: Notify::new(),
                shutdown: AsyncMutex::new(()),
            }),
        })
    }

    /// Admit a fresh worker assignment. Launch failures become terminal result data.
    pub async fn spawn(&self, assignment: Assignment) -> Result<RunRef, Error> {
        let options = assignment.worker.validate()?;
        self.admit(
            None,
            Some((options, SessionStart::Fresh)),
            FollowUp {
                prompt: assignment.prompt,
                timeout: assignment.timeout,
            },
        )
    }

    /// Admit a later assignment, rejecting a busy or unavailable logical worker.
    pub async fn follow_up(
        &self,
        session: impl Into<SessionInput>,
        assignment: FollowUp,
    ) -> Result<RunRef, Error> {
        match session.into() {
            SessionInput::Live(session) => self.admit(Some(session), None, assignment),
            SessionInput::Saved(saved) => self.admit_saved(saved, assignment, SessionStart::Resume),
        }
    }

    /// Admit an assignment in a new session branched from saved native history.
    ///
    /// The source's provider and explicit worker configuration are reused. The
    /// resulting worker gets a distinct logical and native identity. Native
    /// startup or history failures become terminal result data.
    pub async fn fork(&self, source: SavedSession, assignment: FollowUp) -> Result<RunRef, Error> {
        if source.native_id.starts_with('-') || source.native_id.chars().any(char::is_control) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "invalid native fork source ID",
            ));
        }
        self.admit_saved(source, assignment, SessionStart::Fork)
    }

    fn admit_saved(
        &self,
        saved: SavedSession,
        assignment: FollowUp,
        start: impl FnOnce(String) -> SessionStart,
    ) -> Result<RunRef, Error> {
        if saved.worker.ephemeral
            || saved.native_id.trim().is_empty()
            || saved.native_id.len() > MAX_INPUT
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "saved session requires a native ID and persisted history",
            ));
        }
        let options = validate_worker(saved.worker, Some(saved.native_id.clone()))?;
        self.admit(None, Some((options, start(saved.native_id))), assignment)
    }

    fn admit(
        &self,
        session: Option<SessionRef>,
        fresh: Option<(WorkerOptions, SessionStart)>,
        assignment: FollowUp,
    ) -> Result<RunRef, Error> {
        if assignment.prompt.trim().is_empty() || assignment.prompt.len() > MAX_INPUT {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "prompt must be nonempty and at most 1 MiB",
            ));
        }
        validate_duration(assignment.timeout)?;
        let deadline = assignment
            .timeout
            .map(|duration| {
                TokioInstant::now().checked_add(duration).ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidInput,
                        "assignment duration exceeds clock range",
                    )
                })
            })
            .transpose()?;
        let mut state = self.inner.state.lock().unwrap();
        if state.closing {
            return Err(Error::new(ErrorKind::Closed, "engine is shutting down"));
        }
        if let Some(id) = &session {
            let entry = state.sessions.get(id).ok_or_else(|| {
                Error::new(ErrorKind::UnknownSession, "unknown session in this engine")
            })?;
            if entry.releasing {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "session release is in progress",
                ));
            }
            if entry.worker.unavailable.load(Ordering::Acquire) {
                return Err(Error::new(
                    ErrorKind::UnknownSession,
                    "native worker is unavailable; use saved history in a new worker",
                ));
            }
            if let Some(handle) = entry.worker.handle.lock().unwrap().as_deref()
                && !entry
                    .worker
                    .manager
                    .failure(handle)
                    .is_ok_and(|failure| failure.is_none())
            {
                entry.worker.unavailable.store(true, Ordering::Release);
                return Err(Error::new(
                    ErrorKind::UnknownSession,
                    "native worker failed between assignments",
                ));
            }
            if entry.active.is_some() {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "a run is already active for this session",
                ));
            }
        }
        if state.runs.len() >= self.inner.limits.max_runs {
            return Err(Error::new(
                ErrorKind::Capacity,
                "run retention limit reached",
            ));
        }
        let session = match session {
            Some(id) => id,
            None => {
                if state.sessions.len() >= self.inner.limits.max_sessions {
                    return Err(Error::new(ErrorKind::Capacity, "worker limit reached"));
                }
                let (options, start) = fresh.expect("new worker options");
                let (native, fork_source) = match start {
                    SessionStart::Fresh => (None, None),
                    SessionStart::Resume(id) => (Some(id), None),
                    SessionStart::Fork(id) => (None, Some(id)),
                };
                let id = SessionRef(uuid::Uuid::new_v4().to_string());
                let items = Arc::new(SessionItems::new(id.0.clone(), None));
                state.sessions.insert(
                    id.clone(),
                    Session {
                        worker: Arc::new(Worker {
                            fork_source,
                            options,
                            manager: Manager::with_items(Arc::clone(&items)),
                            handle: Mutex::new(None),
                            native: Mutex::new(native),
                            unavailable: AtomicBool::new(false),
                            cleanup: AsyncMutex::new(None),
                            items,
                        }),
                        active: None,
                        releasing: false,
                    },
                );
                id
            }
        };
        let entry = state.sessions.get_mut(&session).unwrap();
        let worker = Arc::clone(&entry.worker);
        let run = RunRef(uuid::Uuid::new_v4().to_string());
        entry.active = Some(run.clone());
        let snapshot = RunSnapshot {
            run: run.clone(),
            session,
            provider: worker.options.provider,
            state: RunState::Starting,
            saved_session: saved_session(&worker),
            result: None,
            interactions: Vec::new(),
        };
        let (cancel, receiver) = watch::channel(false);
        state.runs.insert(
            run.clone(),
            Run {
                snapshot,
                cancel,
                responses: HashSet::new(),
                accepting_responses: true,
                response_error: None,
            },
        );
        worker.items.record(
            "run_admitted",
            serde_json::json!({"provider":worker.options.provider}),
            Some(&run.0),
        );
        drop(state);
        let inner = Arc::clone(&self.inner);
        let id = run.clone();
        let started = Instant::now();
        self.inner.runtime.spawn(async move {
            let task_worker = Arc::clone(&worker);
            let task_inner = Arc::clone(&inner);
            let task_id = id.clone();
            // The outer owner converts panics into retained failures and cleanup.
            let result = tokio::spawn(async move {
                drive(
                    &task_inner,
                    &task_id,
                    &task_worker,
                    assignment,
                    receiver,
                    deadline,
                )
                .await
            })
            .await;
            let mut result = match result {
                Ok(result) => result,
                Err(error) => {
                    let cleanup = close_worker(&worker, inner.limits.settlement_timeout).await;
                    failed(
                        &worker,
                        RunErrorKind::Protocol,
                        format!("core owner failed: {error}; cleanup: {cleanup:?}"),
                    )
                }
            };
            if let Some(error) = inner.finish_responses(&id).await {
                let cleanup = close_worker(&worker, inner.limits.settlement_timeout).await;
                result = result.fail(RunError::new(
                    RunErrorKind::Protocol,
                    format!("{error}; cleanup: {cleanup:?}"),
                ));
            }
            result.run_id = Some(id.0.clone());
            result.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
            let mut state = inner.state.lock().unwrap();
            let session = state.runs[&id].snapshot.session.clone();
            let entry = state.sessions.get_mut(&session).unwrap();
            entry.active = None;
            let snapshot = &mut state.runs.get_mut(&id).unwrap().snapshot;
            worker.items.record(
                "run_finished",
                serde_json::to_value(&result).expect("serializable result"),
                Some(&id.0),
            );
            if worker.unavailable.load(Ordering::Acquire) {
                worker
                    .items
                    .finish(result.error.as_ref().map(|error| error.message.clone()));
            }
            snapshot.state = RunState::Finished;
            snapshot.saved_session = saved_session(&worker);
            snapshot.result = Some(Arc::new(result));
            snapshot.interactions.clear();
            drop(state);
            inner.changed.notify_waiters();
        });
        Ok(run)
    }

    /// Return a non-consuming snapshot, including retained results of older turns.
    pub fn inspect(&self, run: &RunRef) -> Result<RunSnapshot, Error> {
        self.inner.inspect(run)
    }

    /// Subscribe to a full session journal from an exclusive cursor (zero for replay).
    /// Each reader is independent; it never blocks providers or cancels work.
    /// Retained items remain readable after shutdown; late readers can see explicit gaps.
    pub fn subscribe(&self, session: &SessionRef, after: u64) -> Result<SessionStream, Error> {
        let state = self.inner.state.lock().unwrap();
        let entry = state.sessions.get(session).ok_or_else(|| {
            Error::new(ErrorKind::UnknownSession, "unknown session in this engine")
        })?;
        entry.worker.items.validate_cursor(after).map_err(|error| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid session cursor: {error:?}"),
            )
        })?;
        Ok(SessionStream {
            session: session.clone(),
            items: Arc::clone(&entry.worker.items),
            after,
        })
    }

    /// Respond to an exact pending generation using a provider-native JSON object.
    /// Acceptance is not tool completion. Once submitted, dropping this future
    /// leaves response acknowledgement ownership with the core; never auto-retry.
    pub async fn respond(
        &self,
        interaction: &InteractionRef,
        response: serde_json::Value,
    ) -> Result<ResponseReceipt, Error> {
        if serde_json::to_vec(&response).map_or(true, |bytes| bytes.len() > MAX_INPUT) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "native response exceeds 1 MiB",
            ));
        }
        let (worker, handle) = {
            let mut state = self.inner.state.lock().unwrap();
            if state.closing {
                return Err(Error::new(ErrorKind::Closed, "engine is shutting down"));
            }
            let run = state.runs.get(&interaction.run).ok_or_else(|| {
                Error::new(
                    ErrorKind::UnknownRun,
                    "unknown interaction run in this engine",
                )
            })?;
            if run.snapshot.result.is_some() || !run.accepting_responses {
                return Err(Error::new(
                    ErrorKind::UnknownInteraction,
                    "run is finished or retiring",
                ));
            }
            let worker = Arc::clone(&state.sessions[&run.snapshot.session].worker);
            if worker.options.interaction_mode == InteractionMode::Reject {
                return Err(Error::new(
                    ErrorKind::UnknownInteraction,
                    "worker rejects interactions",
                ));
            }
            let handle = worker.handle.lock().unwrap().clone().ok_or_else(|| {
                Error::new(
                    ErrorKind::UnknownInteraction,
                    "worker has no pending interaction",
                )
            })?;
            let pending = worker
                .manager
                .interactions(&handle, &interaction.run.0)
                .map_err(|error| Error::new(ErrorKind::UnknownInteraction, error))?;
            if !pending.iter().any(|(id, cursor, _)| {
                id == &interaction.request_id && *cursor == interaction.cursor
            }) {
                return Err(Error::new(
                    ErrorKind::UnknownInteraction,
                    "interaction is no longer current",
                ));
            }
            Manager::validate_response(worker.options.provider, &response)
                .map_err(|error| Error::new(ErrorKind::InvalidInput, error))?;
            if !state
                .runs
                .get_mut(&interaction.run)
                .unwrap()
                .responses
                .insert(interaction.clone())
            {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "a response for this generation is already pending",
                ));
            }
            (worker, handle)
        };
        let inner = Arc::clone(&self.inner);
        let interaction = interaction.clone();
        let owner = self.inner.runtime.spawn(async move {
            let task_worker = Arc::clone(&worker);
            let task_interaction = interaction.clone();
            let budget = inner.limits.settlement_timeout;
            // The independent owner always releases the reservation, even if the
            // native operation panics. Dropping the caller cannot abandon it.
            let outcome = tokio::spawn(async move {
                timeout(
                    budget,
                    task_worker.manager.reply_checked(
                        &handle,
                        &task_interaction.run.0,
                        &task_interaction.request_id,
                        Some(task_interaction.cursor),
                        response,
                    ),
                )
                .await
            })
            .await;
            let result = match outcome {
                Ok(Ok(Ok(_))) => Ok(ResponseReceipt {
                    interaction: interaction.clone(),
                }),
                Ok(Ok(Err(error))) if stale_reply(&error) => {
                    Err(Error::new(ErrorKind::UnknownInteraction, error))
                }
                Ok(Ok(Err(error))) => Err(Error::new(ErrorKind::ResponseRejected, error)),
                Ok(Err(_)) => Err(Error::new(
                    ErrorKind::ResponseRejected,
                    "native response acknowledgement timed out; delivery unconfirmed",
                )),
                Err(error) => Err(Error::new(
                    ErrorKind::ResponseRejected,
                    format!("native response operation failed; delivery unconfirmed: {error}"),
                )),
            };
            let mut state = inner.state.lock().unwrap();
            let run = state.runs.get_mut(&interaction.run).unwrap();
            if let Err(error) = &result {
                worker.items.record(
                    "interaction_response_failed",
                    serde_json::json!({
                        "request_id": interaction.request_id,
                        "interaction_cursor": interaction.cursor,
                        "message": error.message,
                    }),
                    Some(&interaction.run.0),
                );
                if error.kind == ErrorKind::ResponseRejected {
                    run.response_error
                        .get_or_insert_with(|| error.message.clone());
                    run.cancel.send_replace(true);
                }
            }
            run.responses.remove(&interaction);
            drop(state);
            inner.changed.notify_waiters();
            result
        });
        owner.await.map_err(|error| {
            Error::new(
                ErrorKind::Settlement,
                format!("response owner failed; delivery unconfirmed: {error}"),
            )
        })?
    }

    /// Wait for any/all runs. Expiry or cancellation of this future leaves runs active.
    pub async fn wait(&self, runs: &[RunRef], options: WaitOptions) -> Result<WaitResult, Error> {
        self.inner.wait(runs, options).await
    }

    /// Interrupt this exact assignment and await bounded terminal settlement.
    pub async fn interrupt(&self, run: &RunRef) -> Result<RunSnapshot, Error> {
        {
            let state = self.inner.state.lock().unwrap();
            let entry = state
                .runs
                .get(run)
                .ok_or_else(|| Error::new(ErrorKind::UnknownRun, "unknown run in this engine"))?;
            if entry.snapshot.state == RunState::Finished {
                return Ok(entry.snapshot.clone());
            }
            entry.cancel.send_replace(true);
        }
        let result = self
            .inner
            .wait(
                std::slice::from_ref(run),
                WaitOptions {
                    until: WaitUntil::All,
                    timeout: Some(self.inner.limits.settlement_timeout),
                },
            )
            .await?;
        if !result.condition_met {
            return Err(Error::new(
                ErrorKind::Settlement,
                "interruption is pending; settlement unconfirmed",
            ));
        }
        Ok(result.runs.into_iter().next().unwrap())
    }

    /// Reclaim a finished run's slot while keeping its worker and session journal.
    ///
    /// Collect the result and finish any waits on the run before releasing it.
    /// Subsequent lookups and waits still observing it return `UnknownRun`.
    /// An active run returns `Busy`, and shutdown prevents further release.
    pub fn release_run(&self, run: &RunRef) -> Result<(), Error> {
        let mut state = self.inner.state.lock().unwrap();
        if state.closing {
            return Err(Error::new(ErrorKind::Closed, "engine is shutting down"));
        }
        let entry = state
            .runs
            .get(run)
            .ok_or_else(|| Error::new(ErrorKind::UnknownRun, "unknown run in this engine"))?;
        if entry.snapshot.state != RunState::Finished {
            return Err(Error::new(ErrorKind::Busy, "run is still active"));
        }
        state.runs.remove(run);
        drop(state);
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Settle an idle session's native worker and discard its retained runs and journal.
    ///
    /// Read any results you need first. After success, new lookups by the old
    /// references fail. Existing journal readers can drain to closure, and
    /// saved native history remains available under the provider's own rules.
    /// An active run or concurrent release returns Busy. Cleanup failure
    /// leaves the records and capacity reserved. Dropping the caller's future
    /// after admission leaves settlement with the engine.
    pub async fn release_session(&self, session: &SessionRef) -> Result<(), Error> {
        let worker = {
            let mut state = self.inner.state.lock().unwrap();
            if state.closing {
                return Err(Error::new(ErrorKind::Closed, "engine is shutting down"));
            }
            let entry = state.sessions.get_mut(session).ok_or_else(|| {
                Error::new(ErrorKind::UnknownSession, "unknown session in this engine")
            })?;
            if entry.active.is_some() || entry.releasing {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "session has an active run or release in progress",
                ));
            }
            entry.releasing = true;
            Arc::clone(&entry.worker)
        };
        let inner = Arc::clone(&self.inner);
        let session = session.clone();
        self.inner
            .runtime
            .spawn(async move {
                let cleanup = close_worker(&worker, inner.limits.settlement_timeout).await;
                worker.items.finish(cleanup.as_ref().err().cloned());
                let mut state = inner.state.lock().unwrap();
                match cleanup {
                    Ok(()) => {
                        state.sessions.remove(&session);
                        state
                            .runs
                            .retain(|_, entry| entry.snapshot.session != session);
                        drop(state);
                        inner.changed.notify_waiters();
                        Ok(())
                    }
                    Err(error) => {
                        state.sessions.get_mut(&session).unwrap().releasing = false;
                        Err(Error::new(ErrorKind::Settlement, error))
                    }
                }
            })
            .await
            .map_err(|error| {
                Error::new(
                    ErrorKind::Settlement,
                    format!("release owner failed; settlement unconfirmed: {error}"),
                )
            })?
    }

    /// Deny new work, cancel active assignments and settle all native workers.
    /// Snapshots remain readable afterward. Repeated calls may retry pending cleanup.
    /// Dropping this future leaves its independent shutdown owner running.
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.inner.request_shutdown();
        let inner = Arc::clone(&self.inner);
        self.inner
            .runtime
            .spawn(async move { inner.shutdown().await })
            .await
            .map_err(|error| {
                Error::new(
                    ErrorKind::Settlement,
                    format!("shutdown owner failed; settlement unconfirmed: {error}"),
                )
            })?
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let inner = Arc::clone(&self.inner);
        inner.request_shutdown();
        self.inner.runtime.spawn(async move {
            let _ = inner.shutdown().await;
        });
    }
}

impl Inner {
    fn request_shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.closing = true;
        for run in state.runs.values() {
            run.cancel.send_replace(true);
        }
    }
    fn inspect(&self, run: &RunRef) -> Result<RunSnapshot, Error> {
        self.state
            .lock()
            .unwrap()
            .runs
            .get(run)
            .map(|entry| entry.snapshot.clone())
            .ok_or_else(|| Error::new(ErrorKind::UnknownRun, "unknown run in this engine"))
    }
    fn update(&self, run: &RunRef, worker: &Worker, state: RunState) {
        let mut owned = self.state.lock().unwrap();
        let snapshot = &mut owned.runs.get_mut(run).unwrap().snapshot;
        snapshot.state = state;
        snapshot.saved_session = saved_session(worker);
        if let Some(handle) = worker.handle.lock().unwrap().as_deref() {
            snapshot.interactions = worker
                .manager
                .interactions(handle, &run.0)
                .unwrap_or_default()
                .into_iter()
                .map(|(request_id, cursor, request)| Interaction {
                    id: InteractionRef {
                        run: run.clone(),
                        request_id,
                        cursor,
                    },
                    request,
                })
                .collect();
        }
        drop(owned);
        self.changed.notify_waiters();
    }

    async fn finish_responses(&self, run: &RunRef) -> Option<String> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                let entry = state.runs.get_mut(run).unwrap();
                entry.accepting_responses = false;
                if entry.responses.is_empty() {
                    return entry.response_error.clone();
                }
            }
            changed.await;
        }
    }
    async fn wait(&self, runs: &[RunRef], options: WaitOptions) -> Result<WaitResult, Error> {
        if runs.is_empty()
            || runs
                .iter()
                .enumerate()
                .any(|(i, run)| runs[..i].contains(run))
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "wait requires distinct, nonempty run references",
            ));
        }
        let deadline = options
            .timeout
            .map(|duration| {
                TokioInstant::now().checked_add(duration).ok_or_else(|| {
                    Error::new(ErrorKind::InvalidInput, "wait duration exceeds clock range")
                })
            })
            .transpose()?;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let snapshots = runs
                .iter()
                .map(|id| self.inspect(id))
                .collect::<Result<Vec<_>, _>>()?;
            let terminal = snapshots
                .iter()
                .filter(|snapshot| snapshot.state == RunState::Finished)
                .count();
            let condition_met = match options.until {
                WaitUntil::Any => terminal != 0,
                WaitUntil::All => terminal == snapshots.len(),
            };
            if condition_met || deadline.is_some_and(|end| TokioInstant::now() >= end) {
                return Ok(WaitResult {
                    condition_met,
                    runs: snapshots,
                });
            }
            tokio::select! { _ = changed => {}, _ = at(deadline) => {} }
        }
    }
    async fn shutdown(self: &Arc<Self>) -> Result<(), Error> {
        let _closing = self.shutdown.lock().await;
        let workers = {
            let mut state = self.state.lock().unwrap();
            state.closing = true;
            for run in state.runs.values() {
                run.cancel.send_replace(true);
            }
            state
                .sessions
                .values()
                .map(|entry| (Arc::clone(&entry.worker), entry.active.clone()))
                .collect::<Vec<_>>()
        };
        let mut tasks = tokio::task::JoinSet::new();
        for (worker, active) in workers {
            let inner = Arc::clone(self);
            let bound = self.limits.settlement_timeout;
            tasks.spawn(async move {
                // Only wait for this worker. Releases already admitted own idle
                // sessions, so they cannot remove the active run being waited on.
                if let Some(run) = active
                    && !inner
                        .wait(
                            std::slice::from_ref(&run),
                            WaitOptions {
                                until: WaitUntil::All,
                                timeout: Some(bound),
                            },
                        )
                        .await
                        .map_err(|error| error.to_string())?
                        .condition_met
                {
                    return Err(format!("shutdown requested; run {} has not settled", run.0));
                }
                let result = close_worker(&worker, bound).await;
                worker.items.finish(result.as_ref().err().cloned());
                result
            });
        }
        let mut errors = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push(error),
                Err(error) => errors.push(error.to_string()),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::Settlement, errors.join("; ")))
        }
    }
}

fn validate_duration(value: Option<Duration>) -> Result<(), Error> {
    if value.is_some_and(|d| d.is_zero() || TokioInstant::now().checked_add(d).is_none()) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "duration must be positive and fit the monotonic clock",
        ));
    }
    Ok(())
}
impl WorkerOptions {
    /// Validate the resident policy and resolve paths without starting a worker.
    pub fn validate(self) -> Result<Self, Error> {
        validate_worker(self, None)
    }
}

fn validate_worker(
    mut worker: WorkerOptions,
    resume: Option<String>,
) -> Result<WorkerOptions, Error> {
    let invocation =
        std::env::current_dir().map_err(|e| Error::new(ErrorKind::InvalidInput, e.to_string()))?;
    for path in [Some(worker.cwd.as_path()), worker.binary.as_deref()]
        .into_iter()
        .flatten()
    {
        if path.to_str().is_none() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "worker paths must be UTF-8",
            ));
        }
    }
    let requested = worker
        .binary
        .clone()
        .unwrap_or_else(|| worker.provider.as_str().into());
    worker.binary = Some(if requested.is_absolute() {
        requested
    } else if requested.components().count() > 1 {
        invocation.join(requested)
    } else {
        which::which(&requested).unwrap_or(requested)
    });
    if worker.native_args.len() > 4096
        || worker
            .model
            .as_ref()
            .is_some_and(|value| value.len() > MAX_INPUT)
        || worker
            .native_args
            .iter()
            .try_fold(0usize, |total, value| total.checked_add(value.len()))
            .is_none_or(|bytes| bytes > MAX_INPUT)
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "native arguments exceed 1 MiB",
        ));
    }
    let mut options = open_options(&worker, resume);
    options
        .validate()
        .map_err(|message| Error::new(ErrorKind::InvalidInput, message))?;
    worker.cwd = options.cwd.unwrap();
    if worker.cwd.to_str().is_none()
        || worker
            .binary
            .as_ref()
            .is_some_and(|path| path.to_str().is_none())
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "resolved worker paths must be UTF-8",
        ));
    }
    Ok(worker)
}
fn open_options(worker: &WorkerOptions, resume: Option<String>) -> OpenOptions {
    OpenOptions {
        provider: worker.provider,
        cwd: Some(worker.cwd.clone()),
        binary: worker.binary.clone(),
        model: worker.model.clone(),
        effort: worker.effort,
        resume,
        ephemeral: worker.ephemeral,
        // The injected core journal is always streamable, without a wire push queue.
        stream: true,
        timeout: None,
        native_args: worker.native_args.clone(),
    }
}
fn saved_session(worker: &Worker) -> Option<SavedSession> {
    if worker.options.ephemeral {
        return None;
    }
    worker
        .native
        .lock()
        .unwrap()
        .clone()
        .map(|native_id| SavedSession {
            native_id,
            worker: worker.options.clone(),
        })
}
fn failed(worker: &Worker, kind: RunErrorKind, message: impl Into<String>) -> RunResult {
    let mut result = RunResult::new(worker.options.provider)
        .with_requested(worker.options.model.as_deref(), worker.options.effort)
        .fail(RunError::new(kind, message));
    result.session_id = worker.native.lock().unwrap().clone();
    result
}
async fn at(deadline: Option<TokioInstant>) {
    match deadline {
        Some(end) => tokio::time::sleep_until(end).await,
        None => std::future::pending().await,
    }
}
async fn close_worker(worker: &Worker, bound: Duration) -> Result<(), String> {
    let mut cleanup = worker.cleanup.lock().await;
    if let Some(result) = &*cleanup {
        return result.clone();
    }
    worker.unavailable.store(true, Ordering::Release);
    let result = match timeout(bound, worker.manager.shutdown()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("native shutdown deadline exceeded; settlement unconfirmed".into()),
    };
    *cleanup = Some(result.clone());
    result
}

async fn drive(
    inner: &Inner,
    id: &RunRef,
    worker: &Worker,
    assignment: FollowUp,
    mut cancel: watch::Receiver<bool>,
    deadline: Option<TokioInstant>,
) -> RunResult {
    let handle = worker.handle.lock().unwrap().clone();
    let handle = match handle {
        Some(id) => id,
        None => {
            #[cfg(target_os = "macos")]
            let _open = {
                if *cancel.borrow() {
                    return before_start(worker, false);
                }
                tokio::select! {
                    biased;
                    _ = cancel.changed() => return before_start(worker, false),
                    _ = at(deadline) => return before_start(worker, true),
                    guard = OPEN_NATIVE.lock() => guard,
                }
            };
            if *cancel.borrow() || deadline.is_some_and(|end| TokioInstant::now() >= end) {
                return before_start(worker, !*cancel.borrow());
            }
            let native = worker.native.lock().unwrap().clone();
            let opened = match worker
                .manager
                .open_with_fork(
                    open_options(&worker.options, native),
                    worker.fork_source.as_deref(),
                )
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    worker.unavailable.store(true, Ordering::Release);
                    return failed(worker, RunErrorKind::Provider, error);
                }
            };
            let handle = opened["session_id"].as_str().unwrap().to_owned();
            *worker.handle.lock().unwrap() = Some(handle.clone());
            if let Some(native) = opened["native_session_id"].as_str()
                && let Err(error) = remember_native(worker, native)
            {
                return stop_failed(inner, worker, RunErrorKind::Protocol, error).await;
            }
            handle
        }
    };
    if *cancel.borrow() || deadline.is_some_and(|end| TokioInstant::now() >= end) {
        let (kind, message) = if *cancel.borrow() {
            (RunErrorKind::Cancelled, "cancelled during startup")
        } else {
            (
                RunErrorKind::Timeout,
                "assignment deadline exceeded during startup",
            )
        };
        return stop_failed(inner, worker, kind, message.into()).await;
    }
    let started = match worker
        .manager
        .start_with_id(&handle, assignment.prompt, Some(id.0.clone()))
        .await
    {
        Ok(value) => value,
        Err(error) => return stop_failed(inner, worker, RunErrorKind::Provider, error).await,
    };
    let turn = started["turn_id"].as_str().unwrap();
    inner.update(id, worker, RunState::Running);
    let mut revision = None;
    loop {
        let observed = tokio::select! {
            biased;
            _ = cancel.changed() => return stop_run(inner, worker, &handle, turn, false).await,
            _ = at(deadline) => return stop_run(inner, worker, &handle, turn, true).await,
            observed = worker.manager.observe(&handle, turn, revision) => observed,
        };
        let (observed, seen_revision) = match observed {
            Ok(value) => value,
            Err(error) => return stop_failed(inner, worker, RunErrorKind::Protocol, error).await,
        };
        if let Some(native) = observed["native_session_id"].as_str()
            && let Err(error) = remember_native(worker, native)
        {
            return stop_failed(inner, worker, RunErrorKind::Protocol, error).await;
        }
        revision = Some(seen_revision);
        inner.update(
            id,
            worker,
            if observed["state"] == "awaiting_input" {
                RunState::AwaitingInput
            } else {
                RunState::Running
            },
        );
        if observed["state"] == "finished" {
            let mut result = (*worker.manager.result(&handle, turn).unwrap().unwrap()).clone();
            let unconfirmed_fork =
                worker.fork_source.is_some() && worker.native.lock().unwrap().is_none();
            if unconfirmed_fork && matches!(result.status, Status::Succeeded) {
                return stop_failed(
                    inner,
                    worker,
                    RunErrorKind::Protocol,
                    "native fork did not report a new session identity".into(),
                )
                .await;
            }
            if (unconfirmed_fork || observed["session_error"].is_string())
                && let Err(error) = close_worker(worker, inner.limits.settlement_timeout).await
            {
                result = result.fail(RunError::new(
                    RunErrorKind::Protocol,
                    format!("native failure cleanup: {error}"),
                ));
            }
            return result;
        }
        if observed["state"] == "awaiting_input"
            && worker.options.interaction_mode == InteractionMode::Reject
        {
            return stop_failed(
                inner,
                worker,
                RunErrorKind::Unsupported,
                "native interaction rejected by this worker's interaction mode".into(),
            )
            .await;
        }
        if let Some(error) = observed["session_error"].as_str() {
            return stop_failed(inner, worker, RunErrorKind::Provider, error.into()).await;
        }
    }
}
fn stale_reply(message: &str) -> bool {
    matches!(
        message,
        "unknown or finished turn"
            | "unknown or resolved interaction"
            | "interaction is no longer current"
            | "Unknown or resolved interaction"
            | "Unknown or resolved interaction ID"
            | "No turn is waiting for an interaction"
            | "No pending interaction"
    )
}
fn before_start(worker: &Worker, timed_out: bool) -> RunResult {
    worker.unavailable.store(true, Ordering::Release);
    let (kind, message) = if timed_out {
        (
            RunErrorKind::Timeout,
            "assignment deadline exceeded before native startup",
        )
    } else {
        (RunErrorKind::Cancelled, "cancelled before native startup")
    };
    failed(worker, kind, message)
}
fn remember_native(worker: &Worker, native: &str) -> Result<(), String> {
    if worker.fork_source.as_deref() == Some(native) {
        return Err("native fork reused the source session identity".into());
    }
    let mut known = worker.native.lock().unwrap();
    if native.trim().is_empty() || known.as_ref().is_some_and(|previous| previous != native) {
        return Err("native session identity changed unexpectedly".into());
    }
    *known = Some(native.into());
    Ok(())
}
async fn stop_failed(
    inner: &Inner,
    worker: &Worker,
    kind: RunErrorKind,
    message: String,
) -> RunResult {
    match close_worker(worker, inner.limits.settlement_timeout).await {
        Ok(()) => failed(worker, kind, message),
        Err(error) => failed(
            worker,
            RunErrorKind::Protocol,
            format!("{message}; cleanup: {error}"),
        ),
    }
}
async fn stop_run(
    inner: &Inner,
    worker: &Worker,
    handle: &str,
    turn: &str,
    timed_out: bool,
) -> RunResult {
    // Completion wins when it was already observed before interruption dispatch.
    if let Ok(Some(result)) = worker.manager.result(handle, turn) {
        return (*result).clone();
    }
    let bound = inner.limits.settlement_timeout;
    let stopped = timeout(bound, async {
        let interrupt = worker.manager.interrupt(handle, turn).await;
        if let Err(error) = interrupt {
            if matches!(
                error.as_str(),
                "turn is already finished"
                    | "No active turn"
                    | "Unknown or finished Due turn to interrupt"
                    | "No active Due turn"
            ) && let Ok(Some(result)) = worker.manager.result(handle, turn)
            {
                return Ok((*result).clone());
            }
            return Err(error);
        }
        worker
            .manager
            .wait_result(handle, turn)
            .await
            .map(|result| (*result).clone())
    })
    .await;
    match stopped {
        Ok(Ok(mut result)) => {
            if timed_out {
                result.status = Status::TimedOut;
                result.error = Some(RunError::new(
                    RunErrorKind::Timeout,
                    "assignment deadline exceeded",
                ));
            }
            result
        }
        other => {
            let previous = worker.manager.result(handle, turn).ok().flatten();
            let cleanup = close_worker(worker, bound).await;
            let message =
                format!("native interruption did not settle: {other:?}; cleanup: {cleanup:?}");
            match previous {
                Some(result) => (*result)
                    .clone()
                    .fail(RunError::new(RunErrorKind::Protocol, message)),
                None => failed(worker, RunErrorKind::Protocol, message),
            }
        }
    }
}
