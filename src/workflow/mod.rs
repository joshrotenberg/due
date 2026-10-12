//! Opt-in bounded workflows; all provider work uses ordinary Due subprocesses.
mod bundle;
mod contract;
#[cfg(feature = "workflow-fennel-spike")]
pub(crate) mod fennel;
mod host;
#[cfg(feature = "workflow-lua-spike")]
mod lua;
mod process;
#[cfg(feature = "workflow-spike")]
mod script;
use bundle::Bundle;
use clap::{Args, Subcommand, ValueEnum};
use host::SharedHost;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const fn trace_limit() -> usize {
    contract::MAX_BYTES
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Language {
    #[cfg(feature = "workflow-spike")]
    Rhai,
    #[cfg(feature = "workflow-lua-spike")]
    Lua,
}
impl Language {
    fn from_path(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|e| e.to_str()) {
            #[cfg(feature = "workflow-spike")]
            Some("rhai") => Ok(Self::Rhai),
            #[cfg(feature = "workflow-lua-spike")]
            Some("lua") => Ok(Self::Lua),
            #[cfg(feature = "workflow-fennel-spike")]
            Some("fnl") => Ok(Self::Lua),
            #[cfg(not(feature = "workflow-fennel-spike"))]
            Some("fnl") => Err("Fennel language unavailable in this build".into()),
            #[cfg(not(feature = "workflow-spike"))]
            Some("rhai") => Err("entry/module language unavailable in this build".into()),
            #[cfg(not(feature = "workflow-lua-spike"))]
            Some("lua") => Err("entry/module language unavailable in this build".into()),
            _ => Err("entry/module must have .rhai, .lua or .fnl extension".into()),
        }
    }
    fn patterns(self) -> &'static str {
        match self {
            #[cfg(feature = "workflow-spike")]
            Self::Rhai => include_str!("../../assets/workflow/patterns-v1.rhai"),
            #[cfg(feature = "workflow-lua-spike")]
            Self::Lua => include_str!("../../assets/workflow/patterns-v1.lua"),
        }
    }
    fn check(self, bundle: &Bundle) -> Result<(), String> {
        match self {
            #[cfg(feature = "workflow-spike")]
            Self::Rhai => script::compile(
                &script::engine(None, Arc::new(AtomicBool::new(false))),
                bundle,
            )
            .map(|_| ()),
            #[cfg(feature = "workflow-lua-spike")]
            Self::Lua => lua::check(bundle),
        }
    }
    async fn run(self, host: SharedHost) -> Result<Value, String> {
        match self {
            #[cfg(feature = "workflow-spike")]
            Self::Rhai => {
                let runtime = tokio::runtime::Handle::current();
                tokio::task::spawn_blocking(move || script::run(host, runtime))
                    .await
                    .map_err(|e| format!("Rhai worker: {e}"))?
            }
            #[cfg(feature = "workflow-lua-spike")]
            Self::Lua => lua::run(host).await,
        }
    }
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
    if matches!(args.command, WorkflowCommand::Check(_)) {
        let (value, code) = execute_inner(args, cancelled, interrupted).await;
        return emit(value, code);
    }
    let signal = match crate::shutdown_signal() {
        Ok(signal) => signal,
        Err(e) => {
            return emit(
                json!({"workflow_version":1,"status":"failed","error":format!("install signals: {e}")}),
                1,
            );
        }
    };
    // Poll Lua directly on the async executor; only Rhai uses a blocking thread.
    let work = execute_inner(args, cancelled.clone(), interrupted.clone());
    tokio::pin!(work);
    let (value, code) = tokio::select! {
        result = &mut work => result,
        _ = signal => {
            interrupted.store(true, Ordering::SeqCst);
            cancelled.store(true, Ordering::SeqCst);
            work.await // Always finish Rust-owned cleanup.
        }
    };
    emit(value, code)
}
fn emit(value: Value, code: u8) -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &value).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(code)
}
async fn execute_inner(
    args: WorkflowArgs,
    cancelled: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
) -> (Value, u8) {
    let (options, artifacts) = match args.command {
        WorkflowCommand::Check(options) => (options, None),
        WorkflowCommand::Run { options, artifacts } => (options, Some(artifacts)),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let mut envelope = json!({"workflow_version":1,"workflow_id":id,"status":"failed","steps":[],"output_present":false});
    let mut created = None;
    let result = async {
        if artifacts.is_some() && !cfg!(unix) { return Err("workflow run spike requires Unix signal cleanup".into()); }
        let bundle = Bundle::load(&options.manifest, &options.input)?;
        #[cfg(feature = "workflow-fennel-spike")]
        let bundle = if artifacts.is_none() {
            // Source reads retain normal OS signals. Only compilation owns a
            // helper that needs explicit cancellation and joining during check.
            let signal = crate::shutdown_signal().map_err(|e| e.to_string())?;
            let compilation = fennel::prepare(bundle, cancelled.clone());
            tokio::pin!(compilation);
            tokio::select! {
                result = &mut compilation => result?,
                _ = signal => {
                    interrupted.store(true, Ordering::SeqCst);
                    cancelled.store(true, Ordering::SeqCst);
                    compilation.await?
                }
            }
        } else { fennel::prepare(bundle, cancelled.clone()).await? };
        let language = bundle.language;
        language.check(&bundle)?;
        let Some(artifacts) = artifacts else {
            return Ok(json!({"status":"checked","dynamic_branches_verified":false,"module_initializers":"not executed during check","schema_dialect":contract::DIALECT}));
        };
        process::private_dir(&artifacts)?;
        let artifacts = artifacts.canonicalize().map_err(|e| e.to_string())?;
        created = Some(artifacts.clone());
        envelope["artifacts"] = json!(artifacts);
        process::save(&artifacts.join("snapshot.json"), &bundle.snapshot)?;
        process::private_dir(&artifacts.join("steps"))?;
        process::private_dir(&artifacts.join("schemas"))?;
        for (name, profile) in &bundle.profiles {
            process::save(&artifacts.join("schemas").join(format!("{name}.json")), &profile.schema)?;
        }
        let events = process::private_file(&artifacts.join("workflow.jsonl"))?;
        let host = SharedHost::new(bundle, artifacts, events, id.clone(), cancelled.clone());
        host.lock().event("started", json!({}))?;
        let evaluated = language.run(host.clone()).await;
        let final_value = host.finish(evaluated).await;
        #[cfg(feature = "workflow-lua-spike")]
        let final_value = final_value.map_err(|error| {
            let h = host.lock();
            let error = lua::map_error(&h.bundle, &error);
            match &h.script_location {
                Some(site) => format!("{error}\nlast workflow location: {site}"),
                None => error,
            }
        });
        let final_value = if interrupted.load(Ordering::SeqCst) && host.lock().fatal_cause().is_none() {
            Err("shutdown signal received; active Due children joined".into())
        } else {
            final_value
        };
        envelope["steps"] = json!(host.lock().steps());
        if let Err(error) = &final_value {
            let _ = host.lock().event("stopped", json!({"error":error,"signal":interrupted.load(Ordering::SeqCst)}));
        }
        let value = final_value?;
        Ok::<_, String>(json!({"status":"succeeded","output_present":true,"output":value,"output_validation":{"valid":true}}))
    }.await;
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
            1
        }
    };
    if interrupted.load(Ordering::SeqCst) {
        envelope["status"] = json!("cancelled");
        // Preserve an earlier fatal cause if one was collected during shutdown.
        if envelope.get("error").is_none() {
            envelope["error"] = json!("shutdown signal received; active Due children joined");
        }
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
