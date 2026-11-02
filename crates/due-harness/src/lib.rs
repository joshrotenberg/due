//! Internal workflow runtime shared by `due script` and the language comparison.
//!
//! This crate is not published or a stable Rust client API. JavaScript callers
//! use the documented `Due` scope API; independent clients may use Due's RPC.
//! Call [`run_js`] from a synchronous thread, outside an async executor task.

mod bridge;
#[cfg(feature = "js")]
mod javascript;
mod supervisor;

use bridge::Bridge;
use serde_json::Value;
use std::{
    cell::RefCell,
    io::Write,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use supervisor::Supervisor;

/// Maximum UTF-8 source, argument document, or serialized workflow result size.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Prepared inputs. The caller resolves paths and reads the script beforehand.
pub struct Options {
    /// Absolute path to the Due executable that serves the resident RPC session.
    pub due: PathBuf,
    /// Working directory for Due and caller-owned commands.
    pub cwd: PathBuf,
    /// Fresh directory for exclusive-create evidence files.
    pub evidence: PathBuf,
    /// Trusted JavaScript or comparison-language source, at most one MiB.
    pub source: String,
    /// JSON arguments, at most one MiB; these become `Due.args` in JavaScript.
    pub args: String,
    /// Nonzero whole-workflow execution deadline; cleanup has separate bounds.
    pub timeout: Duration,
}

/// Settled workflow outcome. A result is exposed only after all cleanup succeeds.
#[derive(Debug)]
pub struct Outcome {
    /// JSON return value, or null for an omitted return or any failed workflow.
    pub result: Value,
    /// Workflow, transport, cleanup, or evidence failure. None means success.
    pub error: Option<String>,
    /// Elapsed execution and cleanup time in milliseconds.
    pub elapsed_ms: u64,
}

/// Internal language bridge. Public only to share lifecycle with the Lua trial.
pub struct Host {
    // Rust drops fields in declaration order. Commands must be cancelled and
    // joined before Bridge drops their shared Tokio runtime, including unwind.
    supervisor: RefCell<Option<Supervisor>>,
    bridge: RefCell<Bridge>,
    start: Instant,
    deadline: Instant,
    failure: RefCell<Option<String>>,
}

impl Host {
    /// Record a sticky failure, preserving the first cause.
    pub fn fail(&self, message: String) -> String {
        self.failure.borrow_mut().get_or_insert(message).clone()
    }

    /// Check sticky transport, cancellation, and workflow-deadline state.
    pub fn check(&self) -> Result<(), String> {
        if let Some(error) = self.failure.borrow().as_ref() {
            return Err(error.clone());
        }
        if Instant::now() >= self.deadline {
            return Err(self.fail("workflow deadline exceeded".into()));
        }
        self.bridge.borrow().check().map_err(|e| self.fail(e))
    }

    /// Send one bounded raw RPC frame.
    pub fn send(&self, line: &str) -> Result<(), String> {
        self.check()?;
        self.bridge
            .borrow_mut()
            .send(line)
            .map_err(|e| self.fail(e))?;
        self.check()
    }

    /// Read one raw RPC frame; None means timeout, never EOF.
    pub fn recv(&self, milliseconds: u64) -> Result<Option<String>, String> {
        self.check()?;
        if milliseconds > 10_000 {
            return Err(self.fail("receive timeout must be at most 10000 ms".into()));
        }
        let remaining = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let frame = self
            .bridge
            .borrow_mut()
            .recv(milliseconds.min(remaining))
            .map_err(|e| self.fail(e))?;
        self.check()?;
        Ok(frame)
    }

    /// Monotonic milliseconds since workflow setup began.
    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis().min(u64::MAX as u128) as u64
    }

    /// Start one bounded caller-owned command.
    pub fn exec_start(&self, spec: &str) -> Result<u64, String> {
        self.check()?;
        self.supervisor
            .borrow_mut()
            .as_mut()
            .ok_or("external commands require async mode")?
            .start(spec)
    }

    /// Consume a settled command result, remaining usable during failed cleanup.
    pub fn exec_poll(&self, id: u64) -> Result<Option<String>, String> {
        self.supervisor
            .borrow_mut()
            .as_mut()
            .ok_or("external commands require async mode")?
            .poll(id)
    }

    /// Cancel a command, remaining usable after workflow or transport failure.
    pub fn exec_cancel(&self, id: u64) -> Result<(), String> {
        self.supervisor
            .borrow_mut()
            .as_mut()
            .ok_or("external commands require async mode")?
            .cancel(id)
    }
}

/// Run the supported managed JavaScript workflow and settle all owned processes.
///
/// Setup failures return Err. Once evaluation begins, workflow failures appear
/// in [`Outcome::error`] and are recorded in `workflow.json` when possible.
#[cfg(feature = "js")]
pub fn run_js(options: Options) -> Result<Outcome, String> {
    run_with(options, "js", true, javascript::run)
}

/// Evaluate JavaScript with the shared engine, also preserving the legacy sync trial.
#[cfg(feature = "js")]
pub fn evaluate_js(
    host: Rc<Host>,
    source: &str,
    args: &str,
    async_mode: bool,
) -> Result<Value, String> {
    javascript::run(host, source, args, async_mode)
}

/// Shared ownership boundary for the supported engine and historical Lua trial.
/// The supplied evaluator must finish before this call settles commands and Due.
pub fn run_with(
    options: Options,
    language: &str,
    async_mode: bool,
    evaluate: impl FnOnce(Rc<Host>, &str, &str, bool) -> Result<Value, String>,
) -> Result<Outcome, String> {
    if options.source.len() > MAX_DOCUMENT_BYTES {
        return Err("script exceeds 1 MiB".into());
    }
    if options.args.len() > MAX_DOCUMENT_BYTES {
        return Err("arguments exceed 1 MiB".into());
    }
    serde_json::from_str::<Value>(&options.args)
        .map_err(|e| format!("invalid arguments JSON: {e}"))?;
    for (name, path) in [
        ("Due binary", &options.due),
        ("working directory", &options.cwd),
        ("evidence directory", &options.evidence),
    ] {
        if path.to_str().is_none() {
            return Err(format!("{name} must have a UTF-8 path"));
        }
    }
    if options.timeout.is_zero() {
        return Err("workflow timeout must be positive".into());
    }
    let start = Instant::now();
    let deadline = start
        .checked_add(options.timeout)
        .ok_or("workflow timeout exceeds this platform's scheduling range")?;
    let bridge = Bridge::spawn(&options.due, &options.cwd, &options.evidence)?;
    let supervisor = if async_mode {
        Some(Supervisor::new(
            bridge.runtime_handle(),
            &options.cwd,
            &options.evidence,
        )?)
    } else {
        None
    };
    let host = Rc::new(Host {
        bridge: RefCell::new(bridge),
        supervisor: RefCell::new(supervisor),
        start,
        deadline,
        failure: RefCell::new(None),
    });
    let evaluated = evaluate(Rc::clone(&host), &options.source, &options.args, async_mode);
    let checked = host.check();
    // Native and verifier ownership outlasts every guest error and caught failure.
    let commands = host
        .supervisor
        .borrow_mut()
        .as_mut()
        .map_or(Ok(()), Supervisor::finish);
    let native_cleanup = host.bridge.borrow_mut().finish();
    let cleanup = match (commands, native_cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(native)) => Err(format!("{error}; Due cleanup: {native}")),
    };
    let result = match (checked.and(evaluated), cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(format!("cleanup: {error}")),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}")),
    };
    let mut outcome = match result {
        Ok(result) => Outcome {
            result,
            error: None,
            elapsed_ms: host.now_ms(),
        },
        Err(error) => Outcome {
            result: Value::Null,
            error: Some(error),
            elapsed_ms: host.now_ms(),
        },
    };
    let receipt = serde_json::json!({
        "schema_version": 1,
        "language": language,
        "async": async_mode,
        "status": if outcome.error.is_none() { "succeeded" } else { "failed" },
        "error": outcome.error,
        "elapsed_ms": outcome.elapsed_ms,
        "result": outcome.result,
    });
    let mut file_options = std::fs::OpenOptions::new();
    file_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file_options.mode(0o600);
    }
    let evidence = file_options
        .open(options.evidence.join("workflow.json"))
        .and_then(|mut file| writeln!(file, "{receipt}"));
    if let Err(error) = evidence {
        outcome.error = Some(format!(
            "{}write workflow receipt: {error}",
            outcome
                .error
                .take()
                .map(|e| format!("{e}; "))
                .unwrap_or_default()
        ));
        outcome.result = Value::Null;
    }
    Ok(outcome)
}
