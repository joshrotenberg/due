//! Shared job ownership. Interpreters exchange JSON and unforgeable handles only.
use super::{
    bundle::{self, Bundle},
    contract, process,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashSet},
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Notify, task::JoinHandle};

#[derive(Clone, Debug)]
pub struct RunHandle {
    owner: String,
    index: usize,
}
impl RunHandle {
    fn checked_index(&self, owner: &str, runs: usize) -> Result<usize, String> {
        if self.owner != owner || self.index >= runs {
            return Err("foreign run handle".into());
        }
        Ok(self.index)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    id: String,
    profile: String,
    prompt: String,
}
struct Scope {
    parent: Option<usize>,
    group: String,
    component: usize,
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    depth: usize,
    phase: String,
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    closed: bool,
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    location: Option<String>,
}
struct Run {
    scope: usize,
    job: Job,
    group: String,
    phase: String,
    cancel: Arc<AtomicBool>,
    outcome: Option<Value>,
}
pub struct Host {
    pub bundle: Bundle,
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    pub script_location: Option<String>,
    pub artifacts: PathBuf,
    events: File,
    event_bytes: usize,
    event_count: usize,
    id: String,
    scopes: Vec<Scope>,
    used: HashSet<String>,
    runs: Vec<Run>,
    tasks: Vec<(usize, JoinHandle<()>)>,
    completed: usize,
    pending: BTreeSet<usize>,
    active: usize,
    pub cancelled: Arc<AtomicBool>,
    stop: process::Stop,
}
#[derive(Clone)]
pub struct SharedHost {
    scope: usize,
    state: Arc<Mutex<Host>>,
    changed: Arc<Notify>,
}
impl SharedHost {
    pub fn new(
        bundle: Bundle,
        artifacts: PathBuf,
        events: File,
        id: String,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            scope: 0,
            state: Arc::new(Mutex::new(Host {
                bundle,
                #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
                script_location: None,
                artifacts,
                events,
                event_bytes: 0,
                event_count: 0,
                id,
                scopes: vec![Scope {
                    parent: None,
                    group: "root".into(),
                    component: 0,
                    #[cfg(any(
                        feature = "workflow-lua-spike",
                        feature = "workflow-quickjs-spike"
                    ))]
                    depth: 0,
                    phase: String::new(),
                    #[cfg(any(
                        feature = "workflow-lua-spike",
                        feature = "workflow-quickjs-spike"
                    ))]
                    closed: false,
                    #[cfg(any(
                        feature = "workflow-lua-spike",
                        feature = "workflow-quickjs-spike"
                    ))]
                    location: None,
                }],
                used: HashSet::new(),
                runs: vec![],
                tasks: vec![],
                completed: 0,
                pending: BTreeSet::new(),
                active: 0,
                stop: process::Stop::new(cancelled.clone()),
                cancelled,
            })),
            changed: Arc::new(Notify::new()),
        }
    }
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    pub fn in_scope(&self, scope: usize) -> Self {
        Self {
            scope,
            ..self.clone()
        }
    }
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    pub fn scope_id(&self) -> usize {
        self.scope
    }
    #[cfg(any(
        feature = "workflow-lua-spike",
        all(test, feature = "workflow-quickjs-spike")
    ))]
    pub fn location(&self, location: String, fallback: bool) {
        let mut h = self.lock();
        let site = &mut h.scopes[self.scope].location;
        if !fallback || site.is_none() {
            *site = Some(location);
        }
    }
    #[cfg(feature = "workflow-lua-spike")]
    pub fn scope_failure(&self, error: String) -> String {
        let mut h = self.lock();
        h.fail_at(self.scope, error)
    }
    #[cfg(any(
        feature = "workflow-lua-spike",
        all(test, feature = "workflow-quickjs-spike")
    ))]
    pub fn abandon_group(&self) {
        let mut h = self.lock();
        // A dropped future during signal/collector shutdown is cleanup, not a
        // new failure. In particular, signals must keep their cause-less status.
        if !h.cancelled.load(Ordering::SeqCst) {
            h.fail_at(
                self.scope,
                "concurrent group abandoned before completion".into(),
            );
        }
        self.changed.notify_waiters();
    }
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    pub fn open_scopes(&self, group: &str, count: usize) -> Result<Vec<Self>, String> {
        self.effect(|h| {
            bundle::name(group)?;
            let depth = h.scopes[self.scope].depth + 1;
            if count == 0 || count > 32 {
                return Err("concurrent requires 1..32 functions".into());
            }
            if depth > 8 {
                return Err("concurrent depth exceeds 8".into());
            }
            if h.scopes.len() - 1 + count > 256 {
                return Err("total continuations exceed 256".into());
            }
            if h.scopes.iter().skip(1).filter(|s| !s.closed).count() + count > 64 {
                return Err("active continuations exceed 64".into());
            }
            let mut result = Vec::new();
            for component in 1..=count {
                let scope = h.scopes.len();
                h.scopes.push(Scope {
                    parent: Some(self.scope),
                    group: group.into(),
                    component,
                    depth,
                    phase: String::new(),
                    closed: false,
                    location: None,
                });
                h.scope_event(scope, "scope_started", json!({}))?;
                result.push(self.in_scope(scope));
            }
            Ok(result)
        })
    }
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    pub async fn finish_scope(&self) -> Result<(), String> {
        {
            let mut h = self.lock();
            if !h.cancelled.load(Ordering::SeqCst) {
                if h.has_open_descendants(self.scope) {
                    h.fail_at(
                        self.scope,
                        format!(
                            "scope {} returned with unclosed descendant components",
                            self.scope
                        ),
                    );
                } else if h
                    .runs
                    .iter()
                    .any(|r| r.scope == self.scope && r.outcome.is_none())
                {
                    h.fail_at(
                        self.scope,
                        format!(
                            "scope {} returned with outstanding jobs; cancelling and joining",
                            self.scope
                        ),
                    );
                }
            }
            // On failure, outer finish owns all joins and descendant-first
            // closure. A parent must not finish while its descendants are live.
            h.running()?;
        }
        self.join_scope(Some(self.scope)).await;
        let mut h = self.lock();
        h.running()?;
        h.close_scope(self.scope);
        h.running()
    }
    pub fn lock(&self) -> MutexGuard<'_, Host> {
        self.state.lock().expect("workflow host lock")
    }
    pub fn fail(&self, error: String) -> String {
        self.lock().fail_at(self.scope, error)
    }
    pub fn effect<T>(
        &self,
        action: impl FnOnce(&mut Host) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut h = self.lock();
        h.running()?;
        #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
        if h.scopes[self.scope].closed {
            return Err(h.fail_at(self.scope, "function scope is closed".into()));
        }
        action(&mut h).map_err(|e| h.fail_at(self.scope, e))
    }
    pub fn phase(&self, phase: &str) -> Result<(), String> {
        self.effect(|h| {
            if phase.len() > 256 {
                return Err("phase exceeds 256 bytes".into());
            }
            h.scopes[self.scope].phase = phase.into();
            h.scope_event(self.scope, "phase", json!({}))
        })
    }
    pub fn log(&self, message: &str) -> Result<(), String> {
        self.effect(|h| {
            if message.len() > 4096 {
                return Err("log exceeds 4096 bytes".into());
            }
            h.scope_event(self.scope, "log", json!({"message":message}))
        })
    }
    pub fn start(&self, group: &str, values: Vec<Value>) -> Result<Vec<RunHandle>, String> {
        let mut h = self.lock();
        h.running()?;
        #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
        if h.scopes[self.scope].closed {
            return Err(h.fail_at(self.scope, "function scope is closed".into()));
        }
        let validated = (|| {
            bundle::name(group)?;
            if values.is_empty() || h.used.len() + values.len() > h.bundle.manifest.max_runs {
                return Err("empty batch or max_runs exceeded before admission".into());
            }
            let mut ids = HashSet::new();
            let mut jobs = Vec::new();
            for value in values {
                contract::check_value(&value)?;
                let job: Job = serde_json::from_value(value).map_err(|e| format!("job: {e}"))?;
                bundle::name(&job.id)?;
                if h.used.contains(&job.id) || !ids.insert(job.id.clone()) {
                    return Err(format!("duplicate job id {}", job.id));
                }
                if !h.bundle.profiles.contains_key(&job.profile) {
                    return Err(format!("undeclared profile {}", job.profile));
                }
                if job.prompt.trim().is_empty() || job.prompt.len() > 65536 {
                    return Err("prompt must be nonempty and at most 64 KiB".into());
                }
                jobs.push(job);
            }
            Ok(jobs)
        })();
        let jobs = validated.map_err(|e| h.fail_at(self.scope, e))?;
        // Register the ENTIRE reservation before any fallible receipt or launch.
        let first = h.runs.len();
        for job in jobs {
            h.used.insert(job.id.clone());
            let index = h.runs.len();
            h.pending.insert(index);
            let phase = h.scopes[self.scope].phase.clone();
            h.runs.push(Run {
                scope: self.scope,
                job,
                group: group.into(),
                phase,
                cancel: Arc::new(AtomicBool::new(false)),
                outcome: None,
            });
        }
        let mut handles = Vec::new();
        for index in first..h.runs.len() {
            handles.push(RunHandle {
                owner: h.id.clone(),
                index,
            });
            let run = &h.runs[index];
            let dir = h.artifacts.join("steps").join(&run.job.id);
            let setup = process::private_dir(&dir).and_then(|_| process::save(&dir.join("admitted.json"), &json!({"id":run.job.id,"profile":run.job.profile,"group":run.group,"phase":run.phase,"scope":h.identity(run.scope)})));
            let fields = json!({"step_id":run.job.id,"profile":run.job.profile,"group":run.group});
            let setup = setup.and_then(|_| h.scope_event(self.scope, "admitted", fields));
            if let Err(error) = &setup {
                h.fail(error.clone());
            }
            let run = &h.runs[index];
            let profile = &h.bundle.profiles[&run.job.profile];
            let mut request = profile.request.clone();
            request["schema"] = json!(
                h.artifacts
                    .join("schemas")
                    .join(format!("{}.json", run.job.profile))
            );
            request["prompt"] = json!(run.job.prompt);
            let launch = process::Launch {
                scope: h.identity(run.scope),
                dir,
                request,
                binary: profile.binary.clone(),
            };
            let local = run.cancel.clone();
            let stop = h.stop.step(&run.job.id);
            let host = self.clone();
            let task = tokio::spawn(async move {
                let collected = match setup {
                    Err(e) => synthetic(e),
                    Ok(()) => {
                        // Cancellation while queued never acquires permission to spawn.
                        let active = host.acquire(index, &local).await;
                        let value = process::run(launch, stop, local).await;
                        // Keep the slot through outcome recording, so a receipt failure
                        // stops admission before a queued child can use this slot.
                        {
                            let mut h = host.lock();
                            h.record(index, value);
                            if active {
                                h.active -= 1;
                            }
                        }
                        host.changed.notify_waiters();
                        return;
                    }
                };
                {
                    let mut h = host.lock();
                    h.pending.remove(&index);
                    h.record(index, collected);
                }
                host.changed.notify_waiters();
            });
            h.tasks.push((index, task));
        }
        h.running()?;
        Ok(handles)
    }
    async fn acquire(&self, index: usize, local: &AtomicBool) -> bool {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut h = self.lock();
                if local.load(Ordering::SeqCst) || h.cancelled.load(Ordering::SeqCst) {
                    h.pending.remove(&index);
                    self.changed.notify_waiters();
                    return false;
                }
                if h.pending.first() == Some(&index) && h.active < h.bundle.manifest.max_concurrency
                {
                    h.pending.remove(&index);
                    h.active += 1;
                    self.changed.notify_waiters();
                    return true;
                }
            }
            tokio::select! {
                _ = notified => (),
                _ = tokio::time::sleep(Duration::from_millis(10)) => (),
            }
        }
    }
    fn indices(&self, handles: &[RunHandle]) -> Result<Vec<usize>, String> {
        self.effect(|h| {
            if handles.is_empty() {
                return Err("wait_any requires nonempty handles".into());
            }
            let mut seen = HashSet::new();
            for handle in handles {
                let index = handle.checked_index(&h.id, h.runs.len())?;
                if h.runs[index].scope != self.scope {
                    return Err("run handle belongs to another function scope".into());
                }
                if !seen.insert(index) {
                    return Err("duplicate run handle".into());
                }
            }
            Ok(handles.iter().map(|h| h.index).collect())
        })
    }
    pub async fn wait_any(&self, handles: Vec<RunHandle>) -> Result<Value, String> {
        let indices = self.indices(&handles)?;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let h = self.lock();
                if let Some(error) = h.stop.cause() {
                    return Err(error);
                }
                if let Some(outcome) = indices
                    .iter()
                    .filter_map(|i| h.runs[*i].outcome.as_ref())
                    .min_by_key(|o| o["completion_seq"].as_u64())
                {
                    return Ok(outcome.clone());
                }
            }
            notified.await;
        }
    }
    pub async fn wait(&self, handle: RunHandle) -> Result<Value, String> {
        self.wait_any(vec![handle]).await
    }
    pub async fn cancel(&self, handle: RunHandle) -> Result<Value, String> {
        let index = self.indices(std::slice::from_ref(&handle))?[0];
        self.lock().runs[index].cancel.store(true, Ordering::SeqCst);
        self.wait(handle).await
    }
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-spike"))]
    pub async fn parallel(&self, group: &str, jobs: Vec<Value>) -> Result<Value, String> {
        let handles = self.start(group, jobs)?;
        let mut outcomes = Vec::new();
        for h in handles {
            outcomes.push(self.wait(h).await?);
        }
        Ok(Value::Array(outcomes))
    }
    pub async fn finish(&self, evaluated: Result<Value, String>) -> Result<Value, String> {
        {
            let mut h = self.lock();
            // A collector may have raised the stop flag and still be draining.
            // Let it record the originating fatal cause before a secondary VM
            // "stopped" error, or the outstanding-work policy, can mask it.
            if !h.cancelled.load(Ordering::SeqCst) {
                if let Err(e) = &evaluated {
                    h.fail_at(self.scope, e.clone());
                }
                #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
                if h.scopes.iter().skip(1).any(|s| !s.closed) {
                    h.fail_at(
                        self.scope,
                        "script returned with unclosed components".into(),
                    );
                }
                if h.runs.iter().any(|r| r.outcome.is_none()) {
                    h.fail_at(
                        self.scope,
                        "script returned with outstanding jobs; cancelling and joining".into(),
                    );
                }
            }
        }
        self.join_scope(None).await;
        let mut h = self.lock();
        #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
        for scope in (1..h.scopes.len()).rev() {
            if !h.scopes[scope].closed {
                h.close_scope(scope);
            }
        }
        if let Some(error) = h.stop.cause() {
            return Err(error);
        }
        h.running()?;
        let value = evaluated?;
        contract::check_value(&value)?;
        contract::validate(&h.bundle.output_validator, &value)
            .map_err(|e| format!("output: {e}"))?;
        h.event("completed", json!({}))?;
        Ok(value)
    }
    async fn join_scope(&self, scope: Option<usize>) {
        loop {
            let task = {
                let mut h = self.lock();
                h.tasks
                    .iter()
                    .position(|(index, _)| scope.is_none_or(|s| h.runs[*index].scope == s))
                    .map(|i| h.tasks.swap_remove(i))
            };
            let Some((index, task)) = task else { break };
            // Await via a guard: dropping a Lua continuation restores its join
            // handle to the host, whose outer finish still cancels and joins it.
            struct JoinBack {
                host: SharedHost,
                task: Option<(usize, JoinHandle<()>)>,
            }
            impl Drop for JoinBack {
                fn drop(&mut self) {
                    if let Some(task) = self.task.take() {
                        self.host.lock().tasks.push(task);
                    }
                }
            }
            let mut guard = JoinBack {
                host: self.clone(),
                task: Some((index, task)),
            };
            let result = (&mut guard.task.as_mut().unwrap().1).await;
            guard.task.take();
            if let Err(e) = result {
                let mut h = self.lock();
                let error = format!("child supervisor panicked: {e}; cleanup unresolved");
                h.fail(error.clone());
                h.record(index, synthetic(error));
            }
        }
    }
}
fn synthetic(error: String) -> process::Collected {
    process::Collected {
        result: None,
        exit: json!({"code":null,"signal":null,"started":false}),
        error: Some(error.clone()),
        stop_cause: Some(error),
    }
}
impl Host {
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    fn has_open_descendants(&self, scope: usize) -> bool {
        self.scopes.iter().any(|s| {
            if s.closed {
                return false;
            }
            let mut parent = s.parent;
            while let Some(id) = parent {
                if id == scope {
                    return true;
                }
                parent = self.scopes[id].parent;
            }
            false
        })
    }
    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    fn close_scope(&mut self, scope: usize) {
        self.scopes[scope].closed = true;
        if let Err(e) = self.scope_event(scope, "scope_finished", json!({})) {
            self.fail(e);
        }
    }
    pub fn fatal_cause(&self) -> Option<String> {
        self.stop.cause()
    }
    fn running(&self) -> Result<(), String> {
        if let Some(e) = self.stop.cause() {
            return Err(e);
        }
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("workflow stopped".into());
        }
        Ok(())
    }
    fn fail_at(&mut self, scope: usize, error: String) -> String {
        #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
        if self.stop.cause().is_none() {
            self.script_location = self.scopes[scope].location.clone();
        }
        #[cfg(not(feature = "workflow-lua-spike"))]
        let _ = scope;
        self.fail(error)
    }
    fn fail(&mut self, error: String) -> String {
        self.stop.fail(error)
    }
    pub fn steps(&self) -> Vec<Value> {
        // Keep envelope order stable (admission), expose observed arrival separately.
        self.runs.iter().filter_map(|r| r.outcome.clone()).collect()
    }
    fn identity(&self, scope: usize) -> Value {
        let s = &self.scopes[scope];
        json!({"id":scope,"parent":s.parent,"group":s.group,"component":s.component})
    }
    pub fn event(&mut self, event: &str, fields: Value) -> Result<(), String> {
        self.scope_event(0, event, fields)
    }
    fn scope_event(&mut self, scope: usize, event: &str, fields: Value) -> Result<(), String> {
        let phase = self.scopes[scope].phase.clone();
        self.event_at(scope, &phase, event, fields)
    }
    fn event_at(
        &mut self,
        scope: usize,
        phase: &str,
        event: &str,
        fields: Value,
    ) -> Result<(), String> {
        let record = json!({"workflow_version":1,"workflow_id":self.id,"seq":self.event_count,"event":event,"phase":phase,"scope":self.identity(scope),"data":fields});
        let line = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
        if self.event_count >= 2048 || self.event_bytes + line.len() + 1 > contract::MAX_BYTES {
            return Err("workflow event budget exhausted".into());
        }
        self.events
            .write_all(&line)
            .and_then(|_| self.events.write_all(b"\n"))
            .and_then(|_| self.events.flush())
            .map_err(|e| e.to_string())?;
        self.event_count += 1;
        self.event_bytes += line.len() + 1;
        Ok(())
    }
    fn record(&mut self, index: usize, collected: process::Collected) {
        if let Some(cause) = &collected.stop_cause {
            #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
            if self.script_location.is_none() {
                self.script_location = self.scopes[self.runs[index].scope].location.clone();
            }
            self.fail(format!("step {}: {cause}", self.runs[index].job.id));
        }
        let run = &self.runs[index];
        let profile = &self.bundle.profiles[&run.job.profile];
        let raw = collected.result.as_ref();
        let present = raw
            .and_then(|r| r.get("structured_output_present"))
            .and_then(Value::as_bool)
            == Some(true);
        let answer = raw.and_then(|r| r.get("structured_output"));
        let validation = if present {
            answer
                .ok_or("child declares answer present but omits it".to_string())
                .and_then(|a| {
                    contract::validate(&profile.validator, a).and_then(|_| contract::check_value(a))
                })
        } else {
            Err("child structured answer missing".into())
        };
        let execution_ok = collected.exit["code"] == 0
            && raw.is_some_and(|r| {
                r["schema_version"] == 1 && r["status"] == "succeeded" && r["error"].is_null()
            })
            && collected.error.is_none();
        let dir = self.artifacts.join("steps").join(&run.job.id);
        let mut step = json!({"id":run.job.id,"profile":run.job.profile,"group":run.group,"phase":run.phase,"scope":self.identity(run.scope),"completion_seq":self.completed,"cancel_requested":run.cancel.load(Ordering::SeqCst),"ok":execution_ok && validation.is_ok(),"exit":collected.exit,"error":collected.error,"validation":{"valid":validation.is_ok(),"error":validation.err()},"answer_present":present,"result_file":dir.join("result.json"),"stderr_file":dir.join("stderr.txt"),"trace_file":dir.join("trace.jsonl"),"child_run_id":raw.and_then(|r| r.get("run_id"))});
        if step["ok"] == true
            && let Some(answer) = answer
        {
            step["answer"] = answer.clone();
        }
        let scope = run.scope;
        let phase = run.phase.clone();
        let mut receipt = step.clone();
        receipt["child_result"] = collected.result.unwrap_or(Value::Null);
        self.completed += 1;
        self.runs[index].outcome = Some(step.clone());
        // Retain each outcome BEFORE either fallible side effect. Never short circuit siblings.
        for result in [process::save(&dir.join("step.json"), &receipt), self.event_at(scope, &phase, "step_completed", json!({"step_id":step["id"],"completion_seq":step["completion_seq"],"ok":step["ok"],"error":step["error"],"validation":step["validation"],"child_run_id":step["child_run_id"],"trace_file":step["trace_file"]}))] {
            if let Err(e) = result { self.fail(e); }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    pub(crate) fn fixture() -> (SharedHost, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let bundle = Bundle {
            manifest: serde_json::from_value(json!({
                "schema_version":2,"entry":"main.lua","input_schema":"schema.json",
                "output_schema":"schema.json","max_runs":1,"max_concurrency":1,"profiles":{}
            }))
            .unwrap(),
            #[cfg(feature = "workflow-lua-spike")]
            language: super::super::Language::Lua,
            #[cfg(not(feature = "workflow-lua-spike"))]
            language: super::super::Language::JavaScript,
            entry: String::new(),
            entry_name: "main.lua".into(),
            modules: Default::default(),
            module_names: Default::default(),
            #[cfg(feature = "workflow-fennel-spike")]
            source_maps: Default::default(),
            input: Value::Null,
            output_validator: contract::validator(&json!(true)).unwrap(),
            profiles: Default::default(),
            snapshot: Value::Null,
        };
        let host = SharedHost::new(
            bundle,
            dir.path().into(),
            File::create(dir.path().join("events")).unwrap(),
            "internal-test".into(),
            Arc::new(AtomicBool::new(false)),
        );
        (host, dir)
    }

    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    #[tokio::test]
    async fn unclosed_descendants_fail_before_parent_closure_and_outer_finish_rejects_orphans() {
        for finish_parent in [false, true] {
            let (host, dir) = fixture();
            let scopes = host.open_scopes("outer", 2).unwrap();
            let parent = &scopes[0];
            let child = parent.open_scopes("inner", 1).unwrap().remove(0);
            let _grandchild = child.open_scopes("leaf", 1).unwrap();
            // An open sibling tree must not prevent this sibling from finishing.
            scopes[1].finish_scope().await.unwrap();
            if finish_parent {
                parent.location("parent.fnl:7".into(), false);
                let error = parent.finish_scope().await.unwrap_err();
                assert!(error.contains("unclosed descendant"), "{error}");
                assert!(!host.lock().scopes[parent.scope_id()].closed);
                assert_eq!(host.lock().script_location.as_deref(), Some("parent.fnl:7"));
            } else {
                host.location("root.fnl:3".into(), false);
            }
            let error = host.finish(Ok(json!(true))).await.unwrap_err();
            assert!(error.contains("unclosed"), "{error}");
            if !finish_parent {
                assert_eq!(host.lock().script_location.as_deref(), Some("root.fnl:3"));
            }
            assert!(host.lock().scopes.iter().skip(1).all(|s| s.closed));
            let finished: Vec<u64> = std::fs::read_to_string(dir.path().join("events"))
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str::<Value>(s).unwrap())
                .filter(|v| v["event"] == "scope_finished")
                .map(|v| v["scope"]["id"].as_u64().unwrap())
                .collect();
            assert_eq!(finished, vec![2, 4, 3, 1]);
        }
    }

    #[cfg(any(feature = "workflow-lua-spike", feature = "workflow-quickjs-spike"))]
    #[tokio::test]
    async fn dropped_scope_join_returns_task_until_outer_cleanup_before_closure() {
        let (host, dir) = fixture();
        let parent = host.open_scopes("parent", 1).unwrap().remove(0);
        let child = parent.open_scopes("child", 1).unwrap().remove(0);
        let (release, wait) = tokio::sync::oneshot::channel();
        let joined = Arc::new(AtomicBool::new(false));
        let task_joined = joined.clone();
        let task = tokio::spawn(async move {
            // Dropping release on a test panic also releases this owned task.
            let _ = wait.await;
            task_joined.store(true, Ordering::SeqCst);
        });
        {
            let mut h = host.lock();
            // An outcome may exist before its supervisor has fully returned.
            h.runs.push(Run {
                scope: child.scope_id(),
                job: Job {
                    id: "fixture".into(),
                    profile: "fixture".into(),
                    prompt: "fixture".into(),
                },
                group: "run".into(),
                phase: String::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                outcome: Some(json!(true)),
            });
            h.tasks.push((0, task));
        }
        let mut joining = Box::pin(child.finish_scope());
        std::future::poll_fn(|cx| {
            assert!(joining.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(host.lock().tasks.is_empty());
        drop(joining);
        assert_eq!(host.lock().tasks.len(), 1);
        parent.abandon_group();
        let mut finish = Box::pin(host.finish(Ok(json!(true))));
        std::future::poll_fn(|cx| {
            assert!(finish.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(
            !std::fs::read_to_string(dir.path().join("events"))
                .unwrap()
                .contains("scope_finished")
        );
        release.send(()).unwrap();
        assert!(finish.await.unwrap_err().contains("abandoned"));
        assert!(joined.load(Ordering::SeqCst));
        assert!(host.lock().tasks.is_empty());
        let text = std::fs::read_to_string(dir.path().join("events")).unwrap();
        let finished: Vec<_> = text
            .lines()
            .map(|s| serde_json::from_str::<Value>(s).unwrap())
            .filter(|v| v["event"] == "scope_finished")
            .map(|v| v["scope"]["id"].as_u64().unwrap())
            .collect();
        assert_eq!(finished, vec![2, 1]);
    }
    #[test]
    fn handles_reject_other_invocations_and_unadmitted_indices() {
        let handle = RunHandle {
            owner: "invocation-a".into(),
            index: 0,
        };
        assert_eq!(handle.checked_index("invocation-a", 1).unwrap(), 0);
        assert!(handle.checked_index("invocation-b", 1).is_err());
        assert!(handle.checked_index("invocation-a", 0).is_err());
    }
}
