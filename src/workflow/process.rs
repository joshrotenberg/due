//! Subprocess lifecycle. Every admitted child is joined, including cancellation.
use super::contract::MAX_BYTES;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

/// Cover pipe creation through exec, never the child's lifetime. Rust 1.90 on
/// Darwin sets CLOEXEC after pipe(); a concurrent spawn can inherit a sibling's
/// writer in that window and prevent capture EOF even after that sibling exits.
/// All subprocess spawn sites in the experimental host must use this boundary,
/// including spawns that do not themselves create pipes. See the #19 evidence.
#[cfg(target_os = "macos")]
static SPAWN: Mutex<()> = Mutex::new(());

pub(super) fn with_spawn_lock<T>(spawn: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "macos")]
    let _guard = SPAWN.lock().expect("workflow spawn lock");
    spawn()
}

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
    pub scope: Value,
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

/// Sticky first cause, shared by collectors and host receipt/admission errors.
#[derive(Clone)]
pub struct Stop {
    pub cancelled: Arc<AtomicBool>,
    cause: Arc<Mutex<Option<String>>>,
    prefix: String,
}
impl Stop {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            cause: Arc::new(Mutex::new(None)),
            prefix: String::new(),
        }
    }
    pub fn step(&self, id: &str) -> Self {
        Self {
            prefix: format!("step {id}: "),
            ..self.clone()
        }
    }
    pub fn cause(&self) -> Option<String> {
        self.cause.lock().expect("stop cause lock").clone()
    }
    pub fn fail(&self, error: String) -> String {
        let mut cause = self.cause.lock().expect("stop cause lock");
        let first = cause
            .get_or_insert_with(|| format!("{}{error}", self.prefix))
            .clone();
        self.cancelled.store(true, Ordering::SeqCst);
        first
    }
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

fn request_write_failure(error: std::io::Error, stop: &Stop, local: &AtomicBool) -> Failure {
    let message = format!("write Due request: {error}");
    // Killing one child before intake finishes can close its request pipe.
    // Preserve that step's error without cancelling unrelated jobs.
    if error.kind() == std::io::ErrorKind::BrokenPipe
        && (local.load(Ordering::SeqCst) || stop.cancelled.load(Ordering::SeqCst))
    {
        Failure::Protocol(message)
    } else {
        stop.fail(message.clone());
        Failure::Host(message)
    }
}

async fn capture(
    mut reader: impl AsyncRead + Unpin,
    mut file: File,
    stop: Stop,
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
                if let Some(error) = &error {
                    stop.fail(error.clone());
                }
                // Keep draining after truncation/error so cancellation cannot deadlock on a pipe.
            }
            Err(e) => {
                error = Some(format!("child output read: {e}"));
                stop.fail(error.clone().unwrap());
                break;
            }
        }
    }
    (captured, error)
}

pub async fn run(launch: Launch, stop: Stop, local: Arc<AtomicBool>) -> Collected {
    let mut result = Collected {
        result: None,
        exit: json!({"code":null,"signal":null,"started":false}),
        error: None,
        stop_cause: None,
    };
    let outcome = run_inner(&launch, stop.clone(), local, &mut result).await;
    if let Err(error) = outcome {
        result.error = Some(match error {
            Failure::Host(error) => {
                stop.fail(error.clone());
                result.stop_cause.get_or_insert(error.clone());
                error
            }
            Failure::Protocol(error) => error,
            Failure::Cancelled => "cancelled before launch".into(),
        });
    }
    if let Err(e) = save(
        &launch.dir.join("exit.json"),
        &json!({"exit":result.exit,"error":result.error,"scope":launch.scope}),
    ) {
        stop.fail(e.clone());
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
    stop: Stop,
    local: Arc<AtomicBool>,
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
    let mut child = with_spawn_lock(|| {
        // A job may have been cancelled while waiting for the spawn boundary.
        if stop.cancelled.load(Ordering::SeqCst) || local.load(Ordering::SeqCst) {
            return Err(Failure::Cancelled);
        }
        command
            .spawn()
            .map_err(|e| Failure::Host(format!("spawn Due child: {e}")))
    })?;
    let pid = child.id().ok_or_else(|| "child has no PID".to_string())?;
    collected.exit = json!({"started":true,"pid":pid,"code":null,"signal":null});
    // Persist identity while the child is still running, including interrupted runs.
    let mut started = collected.exit.clone();
    started["scope"] = launch.scope.clone();
    let start_error = save(&launch.dir.join("started.json"), &started).err();
    if let Some(error) = &start_error {
        stop.fail(error.clone());
    }
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let bytes = serde_json::to_vec(&launch.request).expect("JSON value");
    let write_stop = stop.clone();
    let write_local = local.clone();
    let write = async move {
        let outcome = stdin.write_all(&bytes).await;
        drop(stdin);
        outcome.map_err(|error| request_write_failure(error, &write_stop, &write_local))
    };
    let wait_cancel = stop.cancelled.clone();
    let wait = async {
        let mut signalled = false;
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tokio::select! {
                status = child.wait() => break status,
                _ = tick.tick() => {
                    if (wait_cancel.load(Ordering::SeqCst) || local.load(Ordering::SeqCst)) && !signalled {
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
        capture(stdout, out, stop.clone()),
        capture(stderr, err, stop)
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
    if let Some(error) = start_error.or(stdout.1).or(stderr.1) {
        collected.stop_cause = Some(error.clone());
        return Err(error.into());
    }
    written?;
    if collected.result.is_none() {
        return Err(Failure::Protocol(
            "Due child did not emit one JSON result; see result.json".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn broken_request_pipe_after_local_cancel_does_not_stop_siblings() {
        for (cancelled, global) in [(true, false), (false, false), (false, true), (true, true)] {
            let (mut writer, reader) = UnixStream::pair().unwrap();
            drop(reader);
            let error = writer.write_all(&vec![0; 60 * 1024]).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
            let stop = Stop::new(Arc::new(AtomicBool::new(global)));
            let failure = request_write_failure(error, &stop, &AtomicBool::new(cancelled));
            assert_eq!(matches!(failure, Failure::Protocol(_)), cancelled || global);
            assert_eq!(stop.cancelled.load(Ordering::SeqCst), !cancelled || global);
            assert_eq!(stop.cause().is_some(), !cancelled && !global);
        }
    }
}
