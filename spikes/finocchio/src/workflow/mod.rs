//! Integrated module/workflow spike. See docs/integrated-workflow.md for the
//! trusted-host boundary and the public CLI/library lifecycle contract.
pub mod config;
mod host;
mod process;
mod runtime;
mod schema;
mod vm;

pub use config::Config;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};
use tokio::sync::Notify;

#[derive(Clone, Default)]
pub struct Cancellation {
    at: Arc<OnceLock<Instant>>,
    notify: Arc<Notify>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.at.get_or_init(Instant::now);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.at.get().is_some()
    }
}
#[derive(Clone, Default)]
struct Stop {
    external: Cancellation,
    cause: Arc<Mutex<Option<(Instant, String)>>>,
    notify: Arc<Notify>,
}
impl Stop {
    fn stopped(&self) -> bool {
        self.external.is_cancelled() || self.cause.lock().unwrap().is_some()
    }
    fn fail(&self, error: String) -> String {
        let first = self
            .cause
            .lock()
            .unwrap()
            .get_or_insert_with(|| (Instant::now(), error))
            .1
            .clone();
        self.notify.notify_waiters();
        first
    }
    fn error(&self) -> Option<String> {
        self.reason().map(|(_, message)| message)
    }
    // Order the first fatal error and explicit cancellation without wall-clock
    // timestamps. A shared cancellation token still does not share local errors.
    fn reason(&self) -> Option<(bool, String)> {
        let cause = self.cause.lock().unwrap();
        match (self.external.at.get(), cause.as_ref()) {
            (Some(cancelled), Some((failed, message))) if failed < cancelled => {
                Some((false, message.clone()))
            }
            (Some(_), _) => Some((true, "parent cancelled".into())),
            (None, Some((_, message))) => Some((false, message.clone())),
            (None, None) => None,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Run,
    Check,
    Describe,
}

/// Owns one foreground runtime thread, not a daemon. Cancellation closes
/// admission and joins every cooperative Due child. Dropping this future also
/// cancels and joins synchronously; use the token and await for async cleanup.
pub async fn execute(
    config: Config,
    mode: Mode,
    input: Value,
    artifacts: Option<&Path>,
    cancellation: Cancellation,
) -> Value {
    let dir = artifacts.map(Path::to_path_buf);
    let token = cancellation.clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("finocchio-owner".into())
        .spawn(move || {
            let output = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    runtime.block_on(execute_owned(config, mode, input, dir.as_deref(), token))
                }
                Err(e) => error("runtime", e.to_string()),
            };
            let _ = send.send(output);
        });
    let thread = match thread {
        Ok(t) => t,
        Err(e) => return error("runtime", e.to_string()),
    };
    let mut owner = Owner {
        thread: Some(thread),
        cancellation,
    };
    let output = receive
        .await
        .unwrap_or_else(|e| error("runtime", e.to_string()));
    if owner.thread.take().unwrap().join().is_err() {
        return error("runtime", "runtime thread panicked".into());
    }
    output
}
struct Owner {
    thread: Option<std::thread::JoinHandle<()>>,
    cancellation: Cancellation,
}
impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.cancellation.cancel();
            let _ = thread.join();
        }
    }
}
async fn execute_owned(
    config: Config,
    mode: Mode,
    input: Value,
    artifacts: Option<&Path>,
    cancellation: Cancellation,
) -> Value {
    let stop = Stop {
        external: cancellation,
        ..Stop::default()
    };
    let config = match config.canonical() {
        Ok(config) => config,
        Err(e) => return error("configuration", e),
    };
    let dir = if mode == Mode::Run {
        let Some(path) = artifacts else {
            return error(
                "configuration",
                "run needs a fresh artifact directory".into(),
            );
        };
        if let Err(e) = process::directory(path) {
            return error("artifacts", e);
        }
        match path.canonicalize() {
            Ok(p) => Some(p),
            Err(e) => return error("artifacts", e.to_string()),
        }
    } else {
        None
    };
    let host = host::Host::new(config, dir.clone(), stop.clone());
    let result = runtime::evaluate(host.clone(), mode, input).await;
    if let Err(e) = &result
        && !stop.stopped()
    {
        stop.fail(e.clone());
    }
    // Lua continuation futures can be discarded now; job joins are host-owned.
    host.finish().await;
    let steps = host.steps();
    let mut result = if let Some((cancelled, e)) = stop.reason() {
        json!({"status":if cancelled {"cancelled"} else {"failed"}, "error":{"category":if cancelled {"cancelled"} else {"workflow"}, "message":e},"cancellation_requested":stop.external.is_cancelled(),"output":null,"steps":steps})
    } else {
        match result {
            Ok(output) => json!({"status":"succeeded","error":null,"output":output,"steps":steps}),
            Err(e) => error("workflow", e),
        }
    };
    if let Some(dir) = dir
        && let Err(e) = process::save(&dir.join("workflow.json"), &result)
    {
        result["status"] = json!("failed");
        result["error"] = json!({"category":"artifacts","message":e});
    }
    result
}
pub fn error(category: &str, message: String) -> Value {
    json!({"status":"failed","error":{"category":category,"message":message},"output":null,"steps":[]})
}
