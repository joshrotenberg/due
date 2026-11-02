//! Supported JavaScript entry point. Provider execution stays behind raw RPC.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

use clap::Args;
use serde::Serialize;
use serde_json::Value;

const MAX_INPUT: u64 = 1024 * 1024;

#[derive(Debug, Args)]
#[command(
    after_help = crate::help::SHORT,
    after_long_help = crate::help::SCRIPT
)]
pub struct ScriptArgs {
    /// UTF-8 JavaScript file, at most 1 MiB.
    #[arg(value_name = "FILE.js")]
    pub script: PathBuf,
    /// Working directory for the RPC server and default host commands.
    #[arg(short = 'C', long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    /// New directory for stream bytes, process receipts and workflow evidence.
    #[arg(long, value_name = "NEW_DIR")]
    pub evidence: PathBuf,
    /// JSON supplied as Due.args (default: {}); at most 1 MiB.
    #[arg(long, value_name = "JSON", conflicts_with = "args_file")]
    pub args: Option<String>,
    /// Read JSON arguments from a regular file or stdin; at most 1 MiB.
    #[arg(long, value_name = "FILE|-", conflicts_with = "args")]
    pub args_file: Option<PathBuf>,
    /// Workflow deadline, including awaited operations; bounded cleanup follows.
    #[arg(long, value_name = "DURATION", default_value = "5m", value_parser = crate::cli::parse_timeout)]
    pub timeout: Duration,
}

#[derive(Serialize)]
struct Evidence {
    directory: PathBuf,
    stdout: PathBuf,
    process: PathBuf,
    commands: PathBuf,
    workflow: PathBuf,
}

impl Evidence {
    fn new(directory: PathBuf) -> Self {
        Self {
            stdout: directory.join("stdout.bin"),
            process: directory.join("receipt.json"),
            commands: directory.join("exec.jsonl"),
            workflow: directory.join("workflow.json"),
            directory,
        }
    }
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    cli_version: &'static str,
    status: &'static str,
    result: Value,
    error: Option<String>,
    duration_ms: u64,
    evidence: Option<Evidence>,
}

fn read_bounded(reader: impl Read, label: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {label}: {e}"))?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err(format!("{label} exceeds 1 MiB"));
    }
    String::from_utf8(bytes).map_err(|e| format!("{label} must be UTF-8: {e}"))
}

fn read_file(path: &Path, label: &str) -> Result<String, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Do not block on a FIFO before discovering that it is not a regular file.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file: File = options
        .open(path)
        .map_err(|e| format!("open {label} {}: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| format!("stat {label}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("{label} must be a regular file"));
    }
    if metadata.len() > MAX_INPUT {
        return Err(format!("{label} exceeds 1 MiB"));
    }
    read_bounded(file, label)
}

fn prepare(args: &ScriptArgs, invocation: &Path) -> Result<due_harness::Options, String> {
    for (path, label) in [
        (Some(invocation), "invocation directory"),
        (Some(args.script.as_path()), "script path"),
        (Some(args.evidence.as_path()), "evidence path"),
        (args.cwd.as_deref(), "working directory"),
        (args.args_file.as_deref(), "argument-file path"),
    ] {
        if path.is_some_and(|path| path.to_str().is_none()) {
            return Err(format!("{label} must be UTF-8 for the JSON contract"));
        }
    }
    let cwd = invocation
        .join(args.cwd.as_deref().unwrap_or(Path::new(".")))
        .canonicalize()
        .map_err(|e| format!("resolve working directory: {e}"))?;
    if !cwd.is_dir() {
        return Err("working directory must be a directory".into());
    }
    if cwd.to_str().is_none() {
        return Err("resolved working directory must be UTF-8 for the JSON contract".into());
    }
    let source = read_file(&invocation.join(&args.script), "script")?;
    let input = match (&args.args, &args.args_file) {
        (Some(value), _) => value.clone(),
        (_, Some(path)) if path == Path::new("-") => {
            read_bounded(io::stdin().lock(), "JSON arguments")?
        }
        (_, Some(path)) => read_file(&invocation.join(path), "JSON arguments")?,
        _ => "{}".to_owned(),
    };
    if input.len() as u64 > MAX_INPUT {
        return Err("JSON arguments exceed 1 MiB".into());
    }
    serde_json::from_str::<Value>(&input).map_err(|e| format!("invalid JSON arguments: {e}"))?;
    if Instant::now().checked_add(args.timeout).is_none() {
        return Err("workflow deadline exceeds this platform's scheduling range".into());
    }
    Ok(due_harness::Options {
        due: std::env::current_exe().map_err(|e| format!("locate Due executable: {e}"))?,
        cwd,
        evidence: invocation.join(&args.evidence),
        source,
        args: input,
        timeout: args.timeout,
    })
}

pub async fn execute(args: ScriptArgs) -> ExitCode {
    let start = Instant::now();
    let invocation = std::env::current_dir();
    let evidence = invocation
        .as_ref()
        .ok()
        .filter(|cwd| cwd.to_str().is_some() && args.evidence.to_str().is_some())
        .map(|cwd| Evidence::new(cwd.join(&args.evidence)));
    let mut report = Report {
        schema_version: 1,
        cli_version: env!("CARGO_PKG_VERSION"),
        status: "failed",
        result: Value::Null,
        error: None,
        duration_ms: 0,
        evidence,
    };
    let options = invocation
        .map_err(|e| format!("read invocation directory: {e}"))
        .and_then(|cwd| prepare(&args, &cwd));
    let code = match options {
        Err(error) => {
            report.error = Some(error);
            2
        }
        Ok(options) => {
            // The interpreter and its owned Tokio I/O runtime live on this
            // blocking thread, never nested inside the CLI's async runtime.
            match tokio::task::spawn_blocking(move || due_harness::run_js(options)).await {
                Ok(Ok(outcome)) => {
                    report.error = outcome.error;
                    if report.error.is_none() {
                        report.status = "succeeded";
                        report.result = outcome.result;
                        0
                    } else {
                        1
                    }
                }
                Ok(Err(error)) => {
                    report.error = Some(error);
                    1
                }
                Err(error) => {
                    report.error = Some(format!("JavaScript owner failed: {error}"));
                    1
                }
            }
        }
    };
    report.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let output = (|| -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(&mut stdout, &report)?;
        writeln!(stdout)?;
        stdout.flush()
    })();
    if let Err(error) = output {
        if error.kind() != io::ErrorKind::BrokenPipe {
            let _ = writeln!(
                io::stderr().lock(),
                "due: cannot write script result: {error}"
            );
        }
        return ExitCode::from(1);
    }
    ExitCode::from(code)
}
