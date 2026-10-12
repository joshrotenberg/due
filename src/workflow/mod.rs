//! Opt-in experimental foreground workflows. No provider execution in this module:
//! all model work crosses the ordinary `due run` subprocess boundary.
mod bundle;
mod contract;
mod process;
mod script;

use bundle::Bundle;
use clap::{Args, Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashSet,
    fs::File,
    io::Write,
    path::PathBuf,
    process::ExitCode,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const fn trace_limit() -> usize {
    contract::MAX_BYTES
}

#[derive(Debug, Args)]
pub struct WorkflowArgs {
    #[command(subcommand)]
    command: WorkflowCommand,
}
#[derive(Debug, Subcommand)]
enum WorkflowCommand {
    /// Compile and validate only; no script/module initialization or children.
    Check(Options),
    /// Run a bounded program and retain private per-step receipts (Unix only).
    Run {
        #[command(flatten)]
        options: Options,
        /// New private directory; parent must already exist. Never overwritten.
        #[arg(long)]
        artifacts: PathBuf,
    },
}
#[derive(Debug, Args)]
struct Options {
    manifest: PathBuf,
    /// JSON input; relative paths resolve from the manifest's directory.
    #[arg(long)]
    input: PathBuf,
    #[arg(long, value_enum, default_value = "json")]
    output: JsonOutput,
}
#[derive(Debug, Clone, Copy, ValueEnum)]
enum JsonOutput {
    Json,
}

pub async fn execute(args: WorkflowArgs) -> ExitCode {
    let cancelled = Arc::new(AtomicBool::new(false));
    let interrupted = Arc::new(AtomicBool::new(false));
    let is_run = matches!(args.command, WorkflowCommand::Run { .. });
    if !is_run {
        // Check has no children to clean up. Preserve normal OS signal handling,
        // including while a caller-supplied source is blocked on file I/O.
        let (value, code) = execute_sync(
            args,
            cancelled,
            interrupted,
            tokio::runtime::Handle::current(),
        );
        return emit(value, code);
    }
    // Register all shutdown handlers before the blocking VM can launch anything.
    let signal = match crate::shutdown_signal() {
        Ok(signal) => signal,
        Err(e) => {
            return emit(
                json!({"workflow_version":1,"status":"failed","error":format!("install signals: {e}")}),
                1,
            );
        }
    };
    let handle = tokio::runtime::Handle::current();
    let stop = cancelled.clone();
    let signal_seen = interrupted.clone();
    let mut worker =
        tokio::task::spawn_blocking(move || execute_sync(args, stop, signal_seen, handle));
    let result = tokio::select! {
        result = &mut worker => result,
        _ = signal => {
            interrupted.store(true, Ordering::SeqCst);
            cancelled.store(true, Ordering::SeqCst);
            worker.await // Do not drop the VM or any active child futures.
        }
    };
    match result {
        Ok((value, code)) => emit(value, code),
        Err(e) => emit(
            json!({"workflow_version":1,"status":"failed","error":format!("workflow worker failed: {e}; inspect partial artifacts")}),
            1,
        ),
    }
}
fn emit(value: Value, code: u8) -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &value).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(code)
}

fn execute_sync(
    args: WorkflowArgs,
    cancelled: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    handle: tokio::runtime::Handle,
) -> (Value, u8) {
    let (options, artifacts) = match args.command {
        WorkflowCommand::Check(options) => (options, None),
        WorkflowCommand::Run { options, artifacts } => (options, Some(artifacts)),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let mut envelope = json!({"workflow_version":1,"workflow_id":id,"status":"failed","steps":[],"output_present":false});
    let mut created = None;
    let result = (|| -> Result<Value, String> {
        if artifacts.is_some() && !cfg!(unix) {
            return Err("workflow run spike requires Unix signal cleanup".into());
        }
        let bundle = Bundle::load(&options.manifest, &options.input)?;
        let engine = script::engine(None, cancelled.clone());
        let compiled = script::compile(&engine, &bundle)?;
        let Some(artifacts) = artifacts else {
            return Ok(
                json!({"status":"checked","dynamic_branches_verified":false,"module_initializers":"discarded; functions only","schema_dialect":contract::DIALECT}),
            );
        };
        process::private_dir(&artifacts)?;
        let artifacts = artifacts.canonicalize().map_err(|e| e.to_string())?;
        created = Some(artifacts.clone());
        envelope["artifacts"] = json!(artifacts);
        process::save(&artifacts.join("snapshot.json"), &bundle.snapshot)?;
        process::private_dir(&artifacts.join("steps"))?;
        process::private_dir(&artifacts.join("schemas"))?;
        for (name, profile) in &bundle.profiles {
            process::save(
                &artifacts.join("schemas").join(format!("{name}.json")),
                &profile.schema,
            )?;
        }
        let events = process::private_file(&artifacts.join("workflow.jsonl"))?;
        let host = Rc::new(RefCell::new(Host {
            bundle,
            artifacts,
            events,
            event_bytes: 0,
            event_count: 0,
            id: id.clone(),
            phase: String::new(),
            used: HashSet::new(),
            steps: vec![],
            cancelled: cancelled.clone(),
            handle,
            fatal: None,
        }));
        host.borrow_mut().event("started", json!({}))?;
        let mut engine = script::engine(Some(host.clone()), cancelled.clone());
        script::install_modules(&mut engine, compiled.modules)?;
        let mut scope = rhai::Scope::new();
        scope.push_constant_dynamic("input", contract::to_dynamic(&host.borrow().bundle.input)?);
        let evaluated = engine
            .eval_ast_with_scope::<rhai::Dynamic>(&mut scope, &compiled.entry)
            .map_err(|e| e.to_string());
        // Calls are synchronous joins. Thus no child can outlive this VM result;
        // failures use the same admission-stop flag as a signal.
        if evaluated.is_err() {
            cancelled.store(true, Ordering::SeqCst);
        }
        let mut host = host.borrow_mut();
        envelope["steps"] = json!(host.steps);
        let final_value = evaluated
            .and_then(|v| contract::from_dynamic(&v))
            .and_then(|v| {
                contract::validate(&host.bundle.output_validator, &v)
                    .map_err(|e| format!("output: {e}"))?;
                Ok(v)
            });
        let final_value = match &host.fatal {
            Some(error) => Err(error.clone()),
            None => final_value.and_then(|value| {
                if cancelled.load(Ordering::SeqCst) {
                    Err("workflow cancelled".into())
                } else {
                    Ok(value)
                }
            }),
        };
        if let Err(error) = &final_value {
            // Even when the logging budget itself failed, the final envelope
            // retains the original error and all collected step receipts.
            let _ = host.event(
                "stopped",
                json!({"error":error,"signal":interrupted.load(Ordering::SeqCst)}),
            );
        }
        let value = final_value?;
        host.event("completed", json!({}))?;
        Ok(
            json!({"status":"succeeded","output_present":true,"output":value,"output_validation":{"valid":true}}),
        )
    })();
    let mut code = match result {
        Ok(fields) => {
            envelope
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            0
        }
        Err(error) => {
            envelope["error"] = json!(error);
            // A signal and host cancellation may also leave partial completed steps.
            // VM/contract errors remain failed, not falsely reported as signal exits.
            1
        }
    };
    if interrupted.load(Ordering::SeqCst) {
        envelope["status"] = json!("cancelled");
        envelope["error"] = json!("shutdown signal received; active Due children joined");
        envelope["output_present"] = json!(false);
        envelope.as_object_mut().unwrap().remove("output");
        code = 130;
    }
    if let Some(dir) = created
        && let Err(e) = process::save(&dir.join("workflow.json"), &envelope)
    {
        envelope["status"] = json!("failed");
        envelope["artifact_error"] = json!(e);
        return (envelope, 1);
    }
    (envelope, code)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    id: String,
    profile: String,
    prompt: String,
}

struct Host {
    bundle: Bundle,
    artifacts: PathBuf,
    events: File,
    event_bytes: usize,
    event_count: usize,
    id: String,
    phase: String,
    used: HashSet<String>,
    steps: Vec<Value>,
    cancelled: Arc<AtomicBool>,
    handle: tokio::runtime::Handle,
    fatal: Option<String>,
}
impl Host {
    fn event(&mut self, event: &str, fields: Value) -> Result<(), String> {
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
    fn effect<T>(
        &mut self,
        action: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, Box<rhai::EvalAltResult>> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("workflow stopped".into());
        }
        match action(self) {
            Ok(value) => Ok(value),
            Err(e) => {
                let cause = self.fatal.get_or_insert(e).clone();
                self.cancelled.store(true, Ordering::SeqCst);
                Err(cause.into())
            }
        }
    }
    fn batch(&mut self, group: &str, values: rhai::Array) -> Result<rhai::Array, String> {
        bundle::name(group)?;
        if values.is_empty() || self.used.len() + values.len() > self.bundle.manifest.max_runs {
            return Err("empty batch or max_runs exceeded before admission".into());
        }
        let mut ids = HashSet::new();
        let mut jobs = Vec::new();
        for value in values {
            let job: Job = serde_json::from_value(contract::from_dynamic(&value)?)
                .map_err(|e| format!("job: {e}"))?;
            bundle::name(&job.id)?;
            if self.used.contains(&job.id) || !ids.insert(job.id.clone()) {
                return Err(format!("duplicate job id {}", job.id));
            }
            if !self.bundle.profiles.contains_key(&job.profile) {
                return Err(format!("undeclared profile {}", job.profile));
            }
            if job.prompt.trim().is_empty() || job.prompt.len() > 65536 {
                return Err("prompt must be nonempty and at most 64 KiB".into());
            }
            jobs.push(job);
        }
        // Reserve the complete batch before directory setup or any launch.
        self.used.extend(ids);
        for job in &jobs {
            let dir = self.artifacts.join("steps").join(&job.id);
            process::private_dir(&dir)?;
            process::save(
                &dir.join("admitted.json"),
                &json!({"id":job.id,"profile":job.profile,"group":group,"phase":self.phase}),
            )?;
            self.event(
                "admitted",
                json!({"step_id":job.id,"profile":job.profile,"group":group}),
            )?;
        }
        // Chunking keeps a finite number of children and preserves input order.
        // Every chunk joins before the next starts; there are no detached jobs.
        let mut values = Vec::new();
        for chunk in jobs.chunks(self.bundle.manifest.max_concurrency) {
            let mut launches = Vec::new();
            for job in chunk {
                let profile = &self.bundle.profiles[&job.profile];
                let mut request = profile.request.clone();
                request["schema"] = json!(
                    self.artifacts
                        .join("schemas")
                        .join(format!("{}.json", job.profile))
                );
                request["prompt"] = json!(job.prompt);
                launches.push(process::Launch {
                    dir: self.artifacts.join("steps").join(&job.id),
                    request,
                    binary: profile.binary.clone(),
                });
            }
            let cancelled = self.cancelled.clone();
            let results = self.handle.block_on(async move {
                let mut tasks = Vec::new();
                for launch in launches {
                    tasks.push(tokio::spawn(process::run(launch, cancelled.clone())));
                }
                let mut results = Vec::new();
                for task in tasks {
                    results.push(task.await);
                }
                results
            });
            let mut receipt_error = None;
            for (job, result) in chunk.iter().zip(results) {
                let collected = match result {
                    Ok(collected) => collected,
                    Err(e) => {
                        receipt_error.get_or_insert_with(|| {
                            format!("child supervisor panicked: {e}; cleanup unresolved")
                        });
                        continue;
                    }
                };
                if let Some(cause) = &collected.stop_cause {
                    self.fatal
                        .get_or_insert_with(|| format!("step {}: {cause}", job.id));
                }
                let profile = &self.bundle.profiles[&job.profile];
                let raw = collected.result.as_ref();
                let present = raw
                    .and_then(|r| r.get("structured_output_present"))
                    .and_then(Value::as_bool)
                    == Some(true);
                let answer = raw.and_then(|r| r.get("structured_output"));
                let validation = if present {
                    answer
                        .ok_or("child declares answer present but omits it".to_string())
                        .and_then(|a| contract::validate(&profile.validator, a))
                        .and_then(|_| contract::to_dynamic(answer.unwrap()).map(|_| ()))
                } else {
                    Err("child structured answer missing".into())
                };
                let execution_ok = collected.exit["code"] == 0
                    && raw.is_some_and(|r| {
                        r["schema_version"] == 1
                            && r["status"] == "succeeded"
                            && r["error"].is_null()
                    })
                    && collected.error.is_none();
                let dir = self.artifacts.join("steps").join(&job.id);
                let mut step = json!({"id":job.id,"profile":job.profile,"group":group,"phase":self.phase,"ok":execution_ok && validation.is_ok(),"exit":collected.exit,"error":collected.error,"validation":{"valid":validation.is_ok(),"error":validation.err()},"answer_present":present,"result_file":dir.join("result.json"),"stderr_file":dir.join("stderr.txt"),"trace_file":dir.join("trace.jsonl"),"child_run_id":raw.and_then(|r| r.get("run_id"))});
                if step["ok"] == true
                    && let Some(answer) = answer
                {
                    step["answer"] = answer.clone();
                }
                // Persist the untouched child envelope alongside independent host evidence.
                let mut receipt = step.clone();
                receipt["child_result"] = collected.result.unwrap_or(Value::Null);
                self.steps.push(step.clone());
                // Every child in this chunk has already finished. A failed log,
                // receipt or conversion must not erase its siblings' outcomes.
                for result in [
                    process::save(&dir.join("step.json"), &receipt),
                    self.event("step_completed", json!({"step_id":job.id,"ok":step["ok"],"error":step["error"],"validation":step["validation"],"child_run_id":step["child_run_id"],"trace_file":step["trace_file"]})),
                ] {
                    if let Err(error) = result {
                        receipt_error.get_or_insert(error);
                    }
                }
                match contract::to_dynamic(&step) {
                    Ok(value) => values.push(value),
                    Err(error) => {
                        receipt_error.get_or_insert(error);
                    }
                }
            }
            if let Some(error) = receipt_error {
                return Err(error);
            }
            if let Some(cause) = &self.fatal {
                return Err(cause.clone());
            }
        }
        Ok(values)
    }
}
