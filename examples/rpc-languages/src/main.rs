//! Experimental embedded clients over the unchanged Due RPC wire interface.

mod bridge;
#[cfg(feature = "js")]
mod javascript;
#[cfg(feature = "lua")]
mod lua;
mod supervisor;

use bridge::Bridge;
use clap::Parser;
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use supervisor::Supervisor;

#[derive(Parser)]
#[command(about = "Compare embedded Lua/QuickJS clients on one Rust RPC bridge")]
struct Options {
    /// Embedded runtime: lua or js.
    language: String,
    script: PathBuf,
    /// Absolute path to a Due binary built with --features rpc.
    #[arg(long)]
    due: PathBuf,
    #[arg(long)]
    cwd: PathBuf,
    /// A new directory for original stdout bytes and the process receipt.
    #[arg(long)]
    evidence: PathBuf,
    /// JSON passed verbatim to bridge.args.
    #[arg(long, default_value = "{}")]
    args: String,
    /// Run managed Promise/coroutine workflows with supervised verifier processes.
    #[arg(long = "async")]
    async_mode: bool,
    /// Whole workflow deadline, including RPC waits.
    #[arg(long, default_value_t = 300_000, value_parser = clap::value_parser!(u64).range(1..=2_147_483_647))]
    timeout_ms: u64,
}

pub(crate) struct Host {
    bridge: RefCell<Bridge>,
    supervisor: RefCell<Option<Supervisor>>,
    start: Instant,
    deadline: Instant,
    failure: RefCell<Option<String>>,
}

impl Host {
    fn fail(&self, message: String) -> String {
        self.failure.borrow_mut().get_or_insert(message).clone()
    }

    fn check(&self) -> Result<(), String> {
        if let Some(error) = self.failure.borrow().as_ref() {
            return Err(error.clone());
        }
        if Instant::now() >= self.deadline {
            return Err(self.fail("workflow deadline exceeded".into()));
        }
        self.bridge.borrow().check().map_err(|e| self.fail(e))
    }

    fn send(&self, line: &str) -> Result<(), String> {
        self.check()?;
        self.bridge
            .borrow_mut()
            .send(line)
            .map_err(|e| self.fail(e))?;
        self.check()
    }

    fn recv(&self, milliseconds: u64) -> Result<Option<String>, String> {
        self.check()?;
        if milliseconds > 10_000 {
            return Err(self.fail("receive timeout must be at most 10000 ms".into()));
        }
        let remaining = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        let frame = self
            .bridge
            .borrow_mut()
            .recv(milliseconds.min(remaining))
            .map_err(|e| self.fail(e))?;
        self.check()?;
        Ok(frame)
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn exec_start(&self, spec: &str) -> Result<u64, String> {
        self.check()?;
        self.supervisor
            .borrow_mut()
            .as_mut()
            .ok_or("external commands require --async")?
            .start(spec)
    }

    // Cleanup must remain possible after transport failure or workflow expiry.
    fn exec_poll(&self, id: u64) -> Result<Option<String>, String> {
        self.supervisor
            .borrow_mut()
            .as_mut()
            .ok_or("external commands require --async")?
            .poll(id)
    }

    fn exec_cancel(&self, id: u64) -> Result<(), String> {
        self.supervisor
            .borrow_mut()
            .as_mut()
            .ok_or("external commands require --async")?
            .cancel(id)
    }
}

fn run(options: Options) -> Result<(), String> {
    match options.language.as_str() {
        #[cfg(feature = "lua")]
        "lua" => (),
        #[cfg(feature = "js")]
        "js" => (),
        _ => return Err("language is unavailable in this build (choose lua or js)".into()),
    }
    serde_json::from_str::<serde_json::Value>(&options.args)
        .map_err(|e| format!("invalid --args JSON: {e}"))?;
    let source =
        std::fs::read_to_string(&options.script).map_err(|e| format!("read script: {e}"))?;
    if source.len() > 1024 * 1024 {
        return Err("script exceeds 1 MiB".into());
    }
    let bridge = Bridge::spawn(&options.due, &options.cwd, &options.evidence)?;
    let supervisor = if options.async_mode {
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
        start: Instant::now(),
        deadline: Instant::now() + Duration::from_millis(options.timeout_ms),
        failure: RefCell::new(None),
    });
    let evaluated = match options.language.as_str() {
        #[cfg(feature = "lua")]
        "lua" => lua::run(Rc::clone(&host), &source, &options.args, options.async_mode),
        #[cfg(feature = "js")]
        "js" => javascript::run(Rc::clone(&host), &source, &options.args, options.async_mode),
        _ => unreachable!(),
    };
    let checked = host.check();
    // Always settle the owned process, including on guest exceptions or caught host errors.
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
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(format!("cleanup: {error}")),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}")),
    };
    let receipt = serde_json::json!({
        "language": options.language,
        "async": options.async_mode,
        "status": if result.is_ok() { "succeeded" } else { "failed" },
        "error": result.as_ref().err(),
        "elapsed_ms": host.now_ms(),
    });
    let mut file_options = std::fs::OpenOptions::new();
    file_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file_options.mode(0o600);
    }
    use std::io::Write;
    let evidence = file_options
        .open(options.evidence.join("workflow.json"))
        .and_then(|mut file| writeln!(file, "{receipt}"));
    if let Err(error) = evidence {
        return Err(format!(
            "{}write workflow receipt: {error}",
            result.err().map(|e| format!("{e}; ")).unwrap_or_default()
        ));
    }
    result
}

fn main() {
    if let Err(error) = run(Options::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
