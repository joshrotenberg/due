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
struct Run {
    job: Job,
    group: String,
    phase: String,
    cancel: Arc<AtomicBool>,
    outcome: Option<Value>,
}
pub struct Host {
    pub bundle: Bundle,
    #[cfg(feature = "workflow-lua-spike")]
    pub script_location: Option<String>,
    pub artifacts: PathBuf,
    events: File,
    event_bytes: usize,
    event_count: usize,
    id: String,
    phase: String,
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
            state: Arc::new(Mutex::new(Host {
                bundle,
                #[cfg(feature = "workflow-lua-spike")]
                script_location: None,
                artifacts,
                events,
                event_bytes: 0,
                event_count: 0,
                id,
                phase: String::new(),
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
    pub fn lock(&self) -> MutexGuard<'_, Host> {
        self.state.lock().expect("workflow host lock")
    }
    pub fn fail(&self, error: String) -> String {
        self.lock().fail(error)
    }
    pub fn effect<T>(
        &self,
        action: impl FnOnce(&mut Host) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut h = self.lock();
        h.running()?;
        action(&mut h).map_err(|e| h.fail(e))
    }
    pub fn phase(&self, phase: &str) -> Result<(), String> {
        self.effect(|h| {
            if phase.len() > 256 {
                return Err("phase exceeds 256 bytes".into());
            }
            h.phase = phase.into();
            h.event("phase", json!({}))
        })
    }
    pub fn log(&self, message: &str) -> Result<(), String> {
        self.effect(|h| {
            if message.len() > 4096 {
                return Err("log exceeds 4096 bytes".into());
            }
            h.event("log", json!({"message":message}))
        })
    }
    pub fn start(&self, group: &str, values: Vec<Value>) -> Result<Vec<RunHandle>, String> {
        let mut h = self.lock();
        h.running()?;
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
        let jobs = validated.map_err(|e| h.fail(e))?;
        // Register the ENTIRE reservation before any fallible receipt or launch.
        let first = h.runs.len();
        for job in jobs {
            h.used.insert(job.id.clone());
            let index = h.runs.len();
            h.pending.insert(index);
            let phase = h.phase.clone();
            h.runs.push(Run {
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
            let setup = process::private_dir(&dir).and_then(|_| process::save(&dir.join("admitted.json"), &json!({"id":run.job.id,"profile":run.job.profile,"group":run.group,"phase":run.phase})));
            let fields = json!({"step_id":run.job.id,"profile":run.job.profile,"group":run.group});
            let setup = setup.and_then(|_| h.event("admitted", fields));
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
                    h.fail(e.clone());
                }
                if h.runs.iter().any(|r| r.outcome.is_none()) {
                    h.fail("script returned with outstanding jobs; cancelling and joining".into());
                }
            }
        }
        let tasks = std::mem::take(&mut self.lock().tasks);
        for (index, task) in tasks {
            if let Err(e) = task.await {
                let mut h = self.lock();
                let error = format!("child supervisor panicked: {e}; cleanup unresolved");
                h.fail(error.clone());
                h.record(index, synthetic(error));
            }
        }
        let mut h = self.lock();
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
    fn fail(&mut self, error: String) -> String {
        self.stop.fail(error)
    }
    pub fn steps(&self) -> Vec<Value> {
        // Keep envelope order stable (admission), expose observed arrival separately.
        self.runs.iter().filter_map(|r| r.outcome.clone()).collect()
    }
    pub fn event(&mut self, event: &str, fields: Value) -> Result<(), String> {
        let record = json!({"workflow_version":1,"workflow_id":self.id,"seq":self.event_count,"event":event,"phase":self.phase,"data":fields});
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
        let mut step = json!({"id":run.job.id,"profile":run.job.profile,"group":run.group,"phase":run.phase,"completion_seq":self.completed,"cancel_requested":run.cancel.load(Ordering::SeqCst),"ok":execution_ok && validation.is_ok(),"exit":collected.exit,"error":collected.error,"validation":{"valid":validation.is_ok(),"error":validation.err()},"answer_present":present,"result_file":dir.join("result.json"),"stderr_file":dir.join("stderr.txt"),"trace_file":dir.join("trace.jsonl"),"child_run_id":raw.and_then(|r| r.get("run_id"))});
        if step["ok"] == true
            && let Some(answer) = answer
        {
            step["answer"] = answer.clone();
        }
        let mut receipt = step.clone();
        receipt["child_result"] = collected.result.unwrap_or(Value::Null);
        self.completed += 1;
        self.runs[index].outcome = Some(step.clone());
        // Retain each outcome BEFORE either fallible side effect. Never short circuit siblings.
        for result in [process::save(&dir.join("step.json"), &receipt), self.event("step_completed", json!({"step_id":step["id"],"completion_seq":step["completion_seq"],"ok":step["ok"],"error":step["error"],"validation":step["validation"],"child_run_id":step["child_run_id"],"trace_file":step["trace_file"]}))] {
            if let Err(e) = result { self.fail(e); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RunHandle;
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
