//! Subprocess lifecycle. Every admitted child is joined, including cancellation.
use super::contract::MAX_BYTES;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub fn private_dir(path: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|e| format!("create exclusive directory {}: {e}", path.display()))
}
pub fn private_file(path: &Path) -> Result<File, String> {
    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
        .map_err(|e| format!("create exclusive file {}: {e}", path.display()))
}
pub fn save(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = private_file(path)?;
    serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

pub struct Launch {
    pub dir: PathBuf,
    pub request: Value,
    pub binary: PathBuf,
}
pub struct Collected {
    pub result: Option<Value>,
    pub exit: Value,
    pub error: Option<String>,
    pub stop_cause: Option<String>,
}

enum Failure {
    Host(String),
    Protocol(String),
    Cancelled,
}
impl From<String> for Failure {
    fn from(error: String) -> Self {
        Self::Host(error)
    }
}

async fn capture(
    mut reader: impl AsyncRead + Unpin,
    mut file: File,
    cancelled: Arc<AtomicBool>,
) -> (Vec<u8>, Option<String>) {
    let mut captured = Vec::new();
    let mut error = None;
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => {
                let keep = n.min(MAX_BYTES - captured.len());
                if let Err(e) = file.write_all(&buffer[..keep]) {
                    error = Some(format!("artifact write: {e}"));
                }
                captured.extend_from_slice(&buffer[..keep]);
                if keep < n {
                    error = Some("child output exceeded 1 MiB; artifact truncated".into());
                }
                if error.is_some() {
                    cancelled.store(true, Ordering::SeqCst);
                }
                // Keep draining after truncation/error so cancellation cannot deadlock on a pipe.
            }
            Err(e) => {
                error = Some(format!("child output read: {e}"));
                cancelled.store(true, Ordering::SeqCst);
                break;
            }
        }
    }
    (captured, error)
}

pub async fn run(launch: Launch, cancelled: Arc<AtomicBool>) -> Collected {
    let mut result = Collected {
        result: None,
        exit: json!({"code":null,"signal":null,"started":false}),
        error: None,
        stop_cause: None,
    };
    let outcome = run_inner(&launch, cancelled.clone(), &mut result).await;
    if let Err(error) = outcome {
        result.error = Some(match error {
            Failure::Host(error) => {
                cancelled.store(true, Ordering::SeqCst);
                result.stop_cause.get_or_insert(error.clone());
                error
            }
            Failure::Protocol(error) => error,
            Failure::Cancelled => "cancelled before launch".into(),
        });
    }
    if let Err(e) = save(
        &launch.dir.join("exit.json"),
        &json!({"exit":result.exit,"error":result.error}),
    ) {
        cancelled.store(true, Ordering::SeqCst);
        result.stop_cause.get_or_insert(e.clone());
        result.error = Some(match result.error.take() {
            Some(previous) => format!("{previous}; exit receipt: {e}"),
            None => e,
        });
    }
    result
}

async fn run_inner(
    launch: &Launch,
    cancelled: Arc<AtomicBool>,
    collected: &mut Collected,
) -> Result<(), Failure> {
    let out = private_file(&launch.dir.join("result.json"))?;
    let err = private_file(&launch.dir.join("stderr.txt"))?;
    save(&launch.dir.join("request.json"), &launch.request)?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .args(["run", "--request", "-", "--output", "json", "--trace-file"])
        .arg(launch.dir.join("trace.jsonl"))
        .arg("--binary")
        .arg(&launch.binary)
        .arg("--workflow-trace-limit")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if cancelled.load(Ordering::SeqCst) {
        return Err(Failure::Cancelled);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn Due child: {e}"))?;
    let pid = child.id().ok_or_else(|| "child has no PID".to_string())?;
    collected.exit = json!({"started":true,"pid":pid,"code":null,"signal":null});
    // Persist identity while the child is still running, including interrupted runs.
    let start_error = save(&launch.dir.join("started.json"), &collected.exit).err();
    if start_error.is_some() {
        cancelled.store(true, Ordering::SeqCst);
    }
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let bytes = serde_json::to_vec(&launch.request).expect("JSON value");
    let write_cancel = cancelled.clone();
    let write = async move {
        let outcome = stdin.write_all(&bytes).await;
        drop(stdin);
        if outcome.is_err() {
            write_cancel.store(true, Ordering::SeqCst);
        }
        outcome
    };
    let wait_cancel = cancelled.clone();
    let wait = async {
        let mut signalled = false;
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tokio::select! {
                status = child.wait() => break status,
                _ = tick.tick() => {
                    if wait_cancel.load(Ordering::SeqCst) && !signalled {
                        #[cfg(unix)]
                        unsafe { libc::kill(pid as i32, libc::SIGTERM); }
                        signalled = true;
                    }
                }
            }
        }
    };
    // Never abort these futures on VM error or signal: signal the Due process and
    // let its ordinary wrapper cancellation finish before collecting all pipes.
    let (status, written, stdout, stderr) = tokio::join!(
        wait,
        write,
        capture(stdout, out, cancelled.clone()),
        capture(stderr, err, cancelled)
    );
    let status = status.map_err(|e| format!("wait Due child: {e}"))?;
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    collected.exit = json!({"started":true,"pid":pid,"code":status.code(),"signal":signal});
    collected.result = serde_json::from_slice(&stdout.0).ok();
    if let Some(error) = start_error
        .or(stdout.1)
        .or(stderr.1)
        .or_else(|| written.err().map(|e| format!("write Due request: {e}")))
    {
        collected.stop_cause = Some(error.clone());
        return Err(error.into());
    }
    if collected.result.is_none() {
        return Err(Failure::Protocol(
            "Due child did not emit one JSON result; see result.json".into(),
        ));
    }
    Ok(())
}
