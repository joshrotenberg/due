//! In-process engine bridge for the experimental embedded harnesses.

mod operations;

use std::{
    cell::{Cell, RefCell},
    fs::File,
    io::Read,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::MAX_DOCUMENT_BYTES;
use due_core::{Engine, Limits};
use operations::{App, Failure, Request};
use serde::Serialize;
use serde_json::Value;
use tokio::{runtime::Handle, task::JoinSet};

const MAX_PENDING: usize = 16;
const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;

/// Inputs for one trusted, bounded embedded workflow.
pub struct Options {
    pub source: String,
    pub args: Value,
    pub cwd: PathBuf,
    pub claude_binary: Option<PathBuf>,
    pub codex_binary: Option<PathBuf>,
    pub timeout: Duration,
}

#[derive(Serialize)]
struct Completion {
    id: u32,
    result: Result<Value, Failure>,
}

pub struct Host {
    app: Arc<App>,
    runtime: Handle,
    jobs: RefCell<JoinSet<Completion>>,
    next: Cell<u32>,
    deadline: Instant,
    failure: RefCell<Option<String>>,
}

impl Host {
    pub fn fail(&self, message: String) {
        self.failure.borrow_mut().get_or_insert(message);
    }

    pub fn check(&self) -> Result<(), String> {
        if Instant::now() >= self.deadline {
            self.fail("workflow deadline exceeded".into());
        }
        match &*self.failure.borrow() {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    pub fn start(&self, request: &str) -> Result<u32, String> {
        self.check()?;
        if request.len() > MAX_DOCUMENT_BYTES {
            return Err("operation input exceeds 1 MiB".into());
        }
        let mut jobs = self.jobs.borrow_mut();
        if jobs.len() >= MAX_PENDING {
            return Err("too many pending core operations".into());
        }
        let request: Request = serde_json::from_str(request).map_err(|e| e.to_string())?;
        let id = self
            .next
            .get()
            .checked_add(1)
            .ok_or("operation IDs exhausted")?;
        self.next.set(id);
        let app = Arc::clone(&self.app);
        jobs.spawn_on(
            async move {
                let result = operations::dispatch(&app, request).await;
                Completion { id, result }
            },
            &self.runtime,
        );
        Ok(id)
    }

    pub fn poll(&self) -> Result<Option<String>, String> {
        self.check()?;
        let Some(completed) = self.jobs.borrow_mut().try_join_next() else {
            return Ok(None);
        };
        let completed = completed.map_err(|error| {
            let message = format!("core operation failed: {error}");
            self.fail(message.clone());
            message
        })?;
        let encoded = serde_json::to_string(&completed).map_err(|error| error.to_string())?;
        if encoded.len() <= MAX_REPLY_BYTES {
            return Ok(Some(encoded));
        }
        let error = Completion {
            id: completed.id,
            result: Err(Failure::new(
                "result_too_large",
                "core operation reply exceeds 8 MiB",
            )),
        };
        serde_json::to_string(&error)
            .map(Some)
            .map_err(|e| e.to_string())
    }
}

/// Run one workflow on a synchronous owner thread, outside a Tokio executor.
/// The return value is available after the engine and its workers settle.
pub fn run(
    options: Options,
    evaluate: impl FnOnce(&Rc<Host>, &str, &str) -> Result<Value, String>,
) -> Result<Value, String> {
    if options.source.len() > MAX_DOCUMENT_BYTES {
        return Err("script exceeds 1 MiB".into());
    }
    let args = serde_json::to_string(&options.args).map_err(|error| error.to_string())?;
    if args.len() > MAX_DOCUMENT_BYTES {
        return Err("arguments exceed 1 MiB".into());
    }
    if options.timeout.is_zero() {
        return Err("workflow timeout must be positive".into());
    }
    let deadline = Instant::now()
        .checked_add(options.timeout)
        .ok_or("workflow timeout exceeds this platform's scheduling range")?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let engine = runtime
        .block_on(async { Engine::new(Limits::default()) })
        .map_err(|error| error.to_string())?;
    let host = Rc::new(Host {
        app: Arc::new(App {
            engine,
            cwd: options.cwd,
            claude_binary: options.claude_binary,
            codex_binary: options.codex_binary,
        }),
        runtime: runtime.handle().clone(),
        jobs: RefCell::new(JoinSet::new()),
        next: Cell::new(0),
        deadline,
        failure: RefCell::new(None),
    });
    let evaluated = evaluate(&host, &options.source, &args);
    let result = host.check().and(evaluated);
    let mut jobs = std::mem::take(&mut *host.jobs.borrow_mut());
    let cleanup = runtime.block_on(async {
        jobs.shutdown().await;
        host.app.engine.shutdown().await
    });
    match (result, cleanup) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(format!("cleanup: {cleanup}")),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}")),
    }
}

/// Run the experimental command with a language-specific evaluator.
pub fn run_cli(
    usage: &str,
    env_prefix: &str,
    evaluate: impl FnOnce(Options) -> Result<Value, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let script = PathBuf::from(args.next().ok_or(usage)?);
    let arguments = args.next().unwrap_or_else(|| "{}".into());
    if args.next().is_some() {
        return Err(usage.into());
    }
    let arguments = arguments.to_str().ok_or("arguments must be UTF-8")?;
    if arguments.len() > MAX_DOCUMENT_BYTES {
        return Err("arguments exceed 1 MiB".into());
    }
    if !std::fs::metadata(&script)?.is_file() {
        return Err("script must be a regular file".into());
    }
    let mut source = String::new();
    File::open(script)?
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_string(&mut source)?;
    let result = evaluate(Options {
        source,
        args: serde_json::from_str(arguments)?,
        cwd: std::env::current_dir()?,
        claude_binary: std::env::var_os(format!("{env_prefix}_CLAUDE_BINARY")).map(Into::into),
        codex_binary: std::env::var_os(format!("{env_prefix}_CODEX_BINARY")).map(Into::into),
        timeout: Duration::from_secs(300),
    })?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
