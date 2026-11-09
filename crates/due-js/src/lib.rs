//! Optional embedded JavaScript harness for the public due-core engine.

mod operations;

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use due_core::{Engine, Limits};
use due_harness::{MAX_DOCUMENT_BYTES, evaluate_js_runtime};
use operations::{App, Failure, Request};
use rquickjs::{Ctx, Exception, Function, Object};
use serde::Serialize;
use serde_json::Value;
use tokio::{runtime::Handle, task::JoinSet};

const MAX_PENDING: usize = 16;
const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;

/// Inputs for one trusted, bounded JavaScript workflow.
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

struct Host {
    app: Arc<App>,
    runtime: Handle,
    jobs: RefCell<JoinSet<Completion>>,
    next: Cell<u32>,
    deadline: Instant,
    failure: RefCell<Option<String>>,
}

impl Host {
    fn fail(&self, message: String) {
        self.failure.borrow_mut().get_or_insert(message);
    }

    fn check(&self) -> Result<(), String> {
        if Instant::now() >= self.deadline {
            self.fail("workflow deadline exceeded".into());
        }
        match &*self.failure.borrow() {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn start(&self, request: &str) -> Result<u32, String> {
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

    fn poll(&self) -> Result<Option<String>, String> {
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
pub fn run(options: Options) -> Result<Value, String> {
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
    let check = Rc::clone(&host);
    let fail = Rc::clone(&host);
    let evaluated = evaluate_js_runtime(
        &options.source,
        true,
        move || check.check(),
        move |message| fail.fail(message),
        |ctx| install(ctx, &host, &args),
    );
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

fn install<'js>(ctx: &Ctx<'js>, host: &Rc<Host>, args: &str) -> rquickjs::Result<()> {
    let bridge = Object::new(ctx.clone())?;
    bridge.set("args", args)?;
    let start = Rc::clone(host);
    bridge.set(
        "start",
        Function::new(ctx.clone(), move |ctx: Ctx<'_>, request: String| {
            start
                .start(&request)
                .map_err(|error| Exception::throw_message(&ctx, &error))
        })?,
    )?;
    let poll = Rc::clone(host);
    bridge.set(
        "poll",
        Function::new(ctx.clone(), move |ctx: Ctx<'_>| {
            poll.poll()
                .map_err(|error| Exception::throw_message(&ctx, &error))
        })?,
    )?;
    bridge.set(
        "log",
        Function::new(ctx.clone(), |text: String| eprintln!("{text}"))?,
    )?;
    ctx.globals().set("__dueCore", bridge)?;
    ctx.eval::<(), _>(include_str!("bootstrap.js"))?;
    ctx.globals().remove("__dueCore")?;
    Ok(())
}
