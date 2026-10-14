//! Due CLI adapter, not a Claude/Codex adapter. Ownership/spawn/receipt design
//! adapted from root src/workflow/process.rs; differences documented in docs.
use super::Stop;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::watch,
};

const LIMIT: usize = 1024 * 1024;
#[cfg(target_os = "macos")]
static SPAWN: Mutex<()> = Mutex::new(());
pub fn with_spawn_lock<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "macos")]
    let _lock = SPAWN.lock().expect("spawn mutex");
    f()
}
pub fn directory(path: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("exclusive directory {}: {e}", path.display()))
}
pub fn file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("exclusive file {}: {e}", path.display()))
}
pub fn save(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = file(path)?;
    serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

// Due creates trace files exclusively. Poll its private spool, cancel on excess,
// then retain at most LIMIT bytes. This is NOT a hard live disk quota.
async fn trace_capture(
    path: PathBuf,
    mut file: File,
    stop: Stop,
    mut exited: watch::Receiver<bool>,
) -> (usize, Option<String>) {
    let mut error = None;
    loop {
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.len() > LIMIT as u64
        {
            error = Some("trace exceeded 1 MiB; snapshot truncated".to_string());
            stop.fail(error.clone().unwrap());
        }
        if *exited.borrow() {
            break;
        }
        tokio::select! { _ = exited.changed() => (), _ = tokio::time::sleep(Duration::from_millis(10)) => () }
    }
    let mut bytes = Vec::new();
    match File::open(&path) {
        Ok(raw) => {
            if let Err(e) = raw
                .take(LIMIT as u64)
                .read_to_end(&mut bytes)
                .and_then(|_| file.write_all(&bytes))
            {
                error = Some(format!("trace snapshot: {e}"));
            }
            if let Err(e) = std::fs::remove_file(&path) {
                error = Some(format!("trace spool cleanup: {e}"));
            }
        }
        Err(e) => error = Some(format!("trace unavailable: {e}")),
    }
    (bytes.len(), error)
}
async fn capture(
    mut reader: impl AsyncRead + Unpin,
    mut file: File,
    stop: Stop,
    mut exited: watch::Receiver<bool>,
) -> (Vec<u8>, Option<String>) {
    let mut bytes = Vec::new();
    let mut error = None;
    let mut buffer = [0; 8192];
    let mut deadline = None;
    loop {
        if *exited.borrow() && deadline.is_none() {
            deadline = Some(tokio::time::Instant::now() + Duration::from_millis(300));
        }
        let end =
            deadline.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(end), if deadline.is_some() => {
                error.get_or_insert("pipe still open 300 ms after Due exit; capture incomplete".into());
                break;
            }
            _ = exited.changed(), if deadline.is_none() => (),
            read = reader.read(&mut buffer) => match read {
                Ok(0) => break,
                Ok(n) => {
                    let keep = n.min(LIMIT - bytes.len());
                    if let Err(e) = file.write_all(&buffer[..keep]) { error = Some(format!("capture write: {e}")); }
                    bytes.extend_from_slice(&buffer[..keep]);
                    if keep < n { error = Some("capture exceeded 1 MiB; truncated".into()); }
                    if let Some(e) = &error { stop.fail(e.clone()); }
                    // Continue draining while Due completes cooperative cleanup.
                }
                Err(e) => { error = Some(format!("capture read: {e}")); stop.fail(error.clone().unwrap()); break; }
            }
        }
    }
    (bytes, error)
}

pub struct Launch {
    pub binary: PathBuf,
    pub dir: PathBuf,
    pub request: Value,
}
pub struct Collected {
    pub exit: Value,
    pub result: Option<Value>,
    pub errors: Vec<String>,
    pub cancel_requested: bool,
    pub stopped_before_spawn: bool,
    pub trace_bytes: usize,
}
pub async fn run(launch: Launch, stop: Stop) -> Collected {
    let mut c = Collected {
        exit: json!({"started":false,"code":null,"signal":null}),
        result: None,
        errors: Vec::new(),
        cancel_requested: false,
        stopped_before_spawn: false,
        trace_bytes: 0,
    };
    if let Err(e) = inner(&launch, stop.clone(), &mut c).await {
        c.errors.push(e);
    }
    if let Err(e) = save(
        &launch.dir.join("exit.json"),
        &json!({"exit":c.exit,"errors":c.errors,"cancel_requested":c.cancel_requested,"stopped_before_spawn":c.stopped_before_spawn}),
    ) {
        stop.fail(e.clone());
        c.errors.push(e);
    }
    c
}
async fn inner(launch: &Launch, stop: Stop, c: &mut Collected) -> Result<(), String> {
    let out = file(&launch.dir.join("result.json"))?;
    let err = file(&launch.dir.join("stderr.txt"))?;
    let trace_out = file(&launch.dir.join("trace.jsonl"))?;
    let trace_path = launch.dir.join("due-trace.jsonl");
    save(&launch.dir.join("request.json"), &launch.request)?;
    let mut command = Command::new(&launch.binary);
    command
        .args(["run", "--request", "-", "--output", "json", "--trace-file"])
        .arg(&trace_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = with_spawn_lock(|| {
        if stop.stopped() {
            c.stopped_before_spawn = true;
            return Err("cancelled before spawn".to_string());
        }
        let cwd = Path::new(
            launch.request["cwd"]
                .as_str()
                .ok_or("request cwd missing")?,
        );
        if cwd.canonicalize().map_err(|e| e.to_string())? != cwd {
            return Err("workspace changed before spawn".into());
        }
        if launch.binary.canonicalize().map_err(|e| e.to_string())? != launch.binary {
            return Err("Due binary changed before spawn".into());
        }
        command
            .spawn()
            .map_err(|e| format!("spawn exact Due binary {}: {e}", launch.binary.display()))
    })?;
    let pid = child.id().expect("spawned child PID");
    c.exit = json!({"started":true,"pid":pid,"code":null,"signal":null});
    if let Err(e) = save(
        &launch.dir.join("started.json"),
        &json!({"exit":c.exit,"due_binary":launch.binary}),
    ) {
        stop.fail(e.clone());
        c.errors.push(e);
    }
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let bytes = serde_json::to_vec(&launch.request).map_err(|e| e.to_string())?;
    let (done, ended) = watch::channel(false);
    let mut write_ended = ended.clone();
    let write = async move {
        let result = tokio::select! {
            r = stdin.write_all(&bytes) => r.map_err(|e| format!("write Due request: {e}")),
            _ = write_ended.changed() => Err("Due exited before request write finished".into())
        };
        drop(stdin);
        result
    };
    let wait = async {
        let mut signalled = false;
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        let status = loop {
            tokio::select! {
                status = child.wait() => break status,
                _ = tick.tick() => if stop.stopped() && !signalled {
                    // Signal only Due. Its provider adapter owns native cleanup.
                    unsafe { libc::kill(pid as i32, libc::SIGTERM); }
                    signalled = true;
                }
            }
        };
        let _ = done.send(true);
        (status, signalled)
    };
    let ((status, signalled), written, out, err, trace) = tokio::join!(
        wait,
        write,
        capture(stdout, out, stop.clone(), ended.clone()),
        capture(stderr, err, stop.clone(), ended.clone()),
        trace_capture(trace_path, trace_out, stop.clone(), ended)
    );
    c.cancel_requested = signalled;
    let status = status.map_err(|e| format!("wait Due: {e}"))?;
    c.exit = json!({"started":true,"pid":pid,"code":status.code(),"signal":status.signal()});
    c.result = serde_json::from_slice(&out.0).ok();
    c.trace_bytes = trace.0;
    c.errors
        .extend([written.err(), out.1, err.1, trace.1].into_iter().flatten());
    if c.result.is_none() {
        c.errors
            .push("Due did not emit one JSON result; see result.json".into());
    }
    Ok(())
}
