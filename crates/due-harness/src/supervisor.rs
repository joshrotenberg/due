//! Owned verifier processes shared by the Lua and JavaScript async clients.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStderr, ChildStdout, Command},
    runtime::Handle,
    sync::watch,
    task::JoinHandle,
    time::{Instant, timeout},
};

const MAX_JOBS: usize = 8;
const MAX_SPEC: usize = 64 * 1024;
const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_RECEIPT: usize = 64 * 1024 * 1024;
const GRACE: Duration = Duration::from_millis(200);
const SETTLE: Duration = Duration::from_secs(2);

struct Spec {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    timeout_ms: u64,
    max_output_bytes: usize,
}

impl Spec {
    fn parse(source: &str, default_cwd: &Path) -> Result<Self, String> {
        if source.len() > MAX_SPEC {
            return Err("exec specification exceeds 64 KiB".into());
        }
        let value: Value = serde_json::from_str(source).map_err(|e| format!("exec JSON: {e}"))?;
        let object = value
            .as_object()
            .ok_or("exec specification must be an object")?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "program" | "args" | "cwd" | "timeout_ms" | "max_output_bytes"
            ) {
                return Err(format!("unknown exec field {key:?}"));
            }
        }
        let program = object
            .get("program")
            .and_then(Value::as_str)
            .filter(|p| !p.trim().is_empty() && !p.contains('\0'))
            .ok_or("exec program must be a nonempty string without NUL")?
            .to_owned();
        let args = match object.get("args") {
            None => Vec::new(),
            Some(Value::Array(values)) if values.len() <= 256 => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|v| !v.contains('\0'))
                        .map(str::to_owned)
                        .ok_or_else(|| "exec args must be strings without NUL".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err("exec args must be an array of at most 256 strings".into()),
        };
        let cwd = match object.get("cwd") {
            None => default_cwd.to_owned(),
            Some(Value::String(path)) if !path.trim().is_empty() => default_cwd.join(path),
            _ => return Err("exec cwd must be a nonempty string".into()),
        }
        .canonicalize()
        .map_err(|e| format!("exec cwd: {e}"))?;
        if !cwd.is_dir() {
            return Err("exec cwd must be a directory".into());
        }
        if cwd.to_str().is_none() {
            return Err("exec cwd must have a UTF-8 path".into());
        }
        let number = |key: &str, default, maximum| -> Result<u64, String> {
            match object.get(key) {
                None => Ok(default),
                Some(value) => value
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= maximum)
                    .ok_or_else(|| format!("exec {key} must be an integer in 1..{maximum}")),
            }
        };
        Ok(Self {
            program,
            args,
            cwd,
            timeout_ms: number("timeout_ms", 30_000, 300_000)?,
            max_output_bytes: number("max_output_bytes", 64 * 1024, MAX_OUTPUT as u64)? as usize,
        })
    }

    fn value(&self) -> Value {
        json!({"program":self.program,"args":self.args,"cwd":self.cwd,
            "timeout_ms":self.timeout_ms,"max_output_bytes":self.max_output_bytes})
    }
}

struct Job {
    cancel: watch::Sender<bool>,
    task: JoinHandle<Outcome>,
}

struct Outcome {
    value: Value,
    cleanup_error: Option<String>,
}

struct Journal {
    file: File,
    bytes: usize,
    limit: usize,
    failure: Option<String>,
}

impl Journal {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = if bytes.len() > self.limit.saturating_sub(self.bytes) {
            Err(format!("exec receipt exceeded {} byte limit", self.limit))
        } else {
            self.file
                .write_all(bytes)
                .map_err(|e| format!("write exec receipt: {e}"))
        };
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        } else {
            self.bytes += bytes.len();
        }
        result
    }
}

/// A bounded set of caller-owned commands. Results must be consumed with `poll`.
pub(crate) struct Supervisor {
    runtime: Handle,
    cwd: PathBuf,
    journal: Arc<Mutex<Journal>>,
    jobs: BTreeMap<u64, Job>,
    next_id: u64,
    failure: Option<String>,
    finished: Option<Result<(), String>>,
}

impl Supervisor {
    pub(crate) fn new(runtime: Handle, cwd: &Path, evidence: &Path) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(evidence.join("exec.jsonl"))
            .map_err(|e| format!("create exec receipt: {e}"))?;
        Ok(Self {
            runtime,
            cwd: cwd.to_owned(),
            journal: Arc::new(Mutex::new(Journal {
                file,
                bytes: 0,
                limit: MAX_RECEIPT,
                failure: None,
            })),
            jobs: BTreeMap::new(),
            next_id: 0,
            failure: None,
            finished: None,
        })
    }

    pub(crate) fn start(&mut self, source: &str) -> Result<u64, String> {
        if self.finished.is_some() {
            return Err("exec supervisor is closed".into());
        }
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if self.jobs.len() >= MAX_JOBS {
            return Err("exec limit of 8 pending results reached".into());
        }
        let spec = Spec::parse(source, &self.cwd)?;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("exec ID space exhausted")?;
        let id = self.next_id;
        if let Err(error) = record(
            &self.journal,
            json!({"kind":"exec_started","id":id,"options":spec.value()}),
        ) {
            self.failure = Some(error.clone());
            return Err(error);
        }
        let (cancel, receiver) = watch::channel(false);
        let journal = Arc::clone(&self.journal);
        let task = self.runtime.spawn(async move {
            let mut outcome = execute(id, spec, receiver, &journal).await;
            if let Err(error) = record(
                &journal,
                json!({"kind":"exec_finished","id":id,"result":outcome.value,
                "cleanup_error":outcome.cleanup_error}),
            ) {
                outcome.value["status"] = json!("failed");
                outcome.value["error"] = json!(match outcome.value["error"].as_str() {
                    Some(original) => format!("{original}; {error}"),
                    None => error.clone(),
                });
                outcome.cleanup_error = Some(match outcome.cleanup_error {
                    Some(original) => format!("{original}; {error}"),
                    None => error,
                });
            }
            outcome
        });
        self.jobs.insert(id, Job { cancel, task });
        Ok(id)
    }

    /// Consume a settled result. A pending operation returns `None` without waiting.
    pub(crate) fn poll(&mut self, id: u64) -> Result<Option<String>, String> {
        let job = self.jobs.get(&id).ok_or("unknown or consumed exec ID")?;
        if !job.task.is_finished() {
            return Ok(None);
        }
        let job = self.jobs.remove(&id).unwrap();
        match self.runtime.block_on(job.task) {
            Ok(outcome) => {
                if let Some(error) = outcome.cleanup_error {
                    self.failure = Some(match self.failure.take() {
                        Some(original) => format!("{original}; {error}"),
                        None => error,
                    });
                }
                Ok(Some(outcome.value.to_string()))
            }
            Err(error) => {
                let message = format!("exec owner failed: {error}");
                self.failure = Some(match self.failure.take() {
                    Some(original) => format!("{original}; {message}"),
                    None => message.clone(),
                });
                Err(message)
            }
        }
    }

    pub(crate) fn cancel(&mut self, id: u64) -> Result<(), String> {
        let job = self.jobs.get(&id).ok_or("unknown or consumed exec ID")?;
        job.cancel.send_replace(true);
        Ok(())
    }

    /// Cancel and join every unconsumed operation, even after a guest failure.
    pub(crate) fn finish(&mut self) -> Result<(), String> {
        if let Some(result) = &self.finished {
            return result.clone();
        }
        let mut errors = Vec::new();
        if let Some(error) = self.failure.take() {
            errors.push(error);
        }
        if !self.jobs.is_empty() {
            errors
                .push("workflow abandoned unconsumed exec operations; cancelled and joined".into());
        }
        for job in self.jobs.values() {
            job.cancel.send_replace(true);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(7);
        for (_, mut job) in std::mem::take(&mut self.jobs) {
            let result = self.runtime.block_on(async {
                timeout(
                    deadline.saturating_duration_since(std::time::Instant::now()),
                    &mut job.task,
                )
                .await
            });
            match result {
                Ok(Ok(outcome)) => {
                    if let Some(error) = outcome.cleanup_error {
                        errors.push(error);
                    }
                }
                Ok(Err(error)) => errors.push(format!("exec owner failed: {error}")),
                Err(_) => {
                    errors.push("exec owner exceeded cleanup deadline".into());
                    job.task.abort();
                    let _ = self.runtime.block_on(job.task);
                }
            }
        }
        match self.journal.lock() {
            Ok(mut journal) => {
                if let Some(error) = &journal.failure
                    && !errors.contains(error)
                {
                    errors.push(error.clone());
                }
                if let Err(error) = journal.file.flush() {
                    errors.push(format!("flush exec receipt: {error}"));
                }
            }
            Err(_) => errors.push("exec receipt lock poisoned".into()),
        }
        let result = if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        };
        self.finished = Some(result.clone());
        result
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn record(journal: &Mutex<Journal>, value: Value) -> Result<(), String> {
    let mut bytes = value.to_string().into_bytes();
    bytes.push(b'\n');
    journal
        .lock()
        .map_err(|_| "exec receipt lock poisoned")?
        .write(&bytes)
}

fn result(
    status: &str,
    exit: Option<ExitStatus>,
    output: &Output,
    mut error: Option<String>,
) -> Value {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        exit.and_then(|s| s.signal())
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let mut decode = |bytes: &[u8], name| match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => {
            let message = format!("exec {name} is not valid UTF-8; text unavailable");
            error = Some(error.take().map_or_else(
                || message.clone(),
                |original| format!("{original}; {message}"),
            ));
            String::new()
        }
    };
    let stdout = decode(&output.stdout, "stdout");
    let stderr = decode(&output.stderr, "stderr");
    let status = if error.is_some() { "failed" } else { status };
    json!({"status":status,"exit_code":exit.and_then(|s| s.code()),"signal":signal,
        "stdout":stdout,"stderr":stderr,"error":error})
}

#[derive(Default)]
struct Output {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_done: bool,
    stderr_done: bool,
}

impl Output {
    fn complete(&self) -> bool {
        self.stdout_done && self.stderr_done
    }

    async fn read(
        &mut self,
        stdout: &mut ChildStdout,
        stderr: &mut ChildStderr,
        limit: usize,
    ) -> Result<(), String> {
        let mut out = [0u8; 4096];
        let mut err = [0u8; 4096];
        let (target, read) = tokio::select! {
            count = stdout.read(&mut out), if !self.stdout_done => (true, count.map(|count| (count, out))),
            count = stderr.read(&mut err), if !self.stderr_done => (false, count.map(|count| (count, err))),
            else => return std::future::pending().await,
        };
        let (count, buffer) = match read {
            Ok(read) => read,
            Err(error) => {
                if target {
                    self.stdout_done = true;
                } else {
                    self.stderr_done = true;
                }
                return Err(format!("read exec output: {error}"));
            }
        };
        if count == 0 {
            if target {
                self.stdout_done = true;
            } else {
                self.stderr_done = true;
            }
            return Ok(());
        }
        let available = limit.saturating_sub(self.stdout.len() + self.stderr.len());
        let kept = count.min(available);
        if target {
            self.stdout.extend_from_slice(&buffer[..kept]);
        } else {
            self.stderr.extend_from_slice(&buffer[..kept]);
        }
        if kept != count {
            return Err("exec output exceeded configured byte limit".into());
        }
        Ok(())
    }
}

async fn execute(
    id: u64,
    spec: Spec,
    mut cancel: watch::Receiver<bool>,
    journal: &Mutex<Journal>,
) -> Outcome {
    let mut output = Output::default();
    let failed = |error| Outcome {
        value: result("failed", None, &Output::default(), Some(error)),
        cleanup_error: None,
    };
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    command.kill_on_drop(true);
    let spawned = crate::with_spawn_lock(|| {
        // Cancellation may arrive while another synchronous spawn holds the lock.
        if *cancel.borrow() {
            return Ok(None);
        }
        command
            .spawn()
            .map(Some)
            .map_err(|error| format!("spawn exec: {error}"))
    });
    let mut child = match spawned {
        Ok(Some(child)) => child,
        Ok(None) => {
            return Outcome {
                value: result("cancelled", None, &output, None),
                cleanup_error: None,
            };
        }
        Err(error) => return failed(error),
    };
    let pid = child.id().unwrap();
    let mut custody = Custody::new(pid);
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut error = record(journal, json!({"kind":"exec_spawned","id":id,"pid":pid})).err();
    let deadline = Instant::now() + Duration::from_millis(spec.timeout_ms);
    let mut status = "failed";
    let mut known_exit = None;
    if error.is_none() {
        loop {
            if *cancel.borrow() {
                status = "cancelled";
                break;
            }
            match observe(&mut child, &mut custody, &mut known_exit) {
                Ok(true) => {
                    status = "completed";
                    break;
                }
                Ok(false) => {}
                Err(problem) => {
                    error = Some(problem);
                    break;
                }
            }
            tokio::select! {
                biased;
                _ = cancel.changed() => { status = "cancelled"; break; },
                _ = tokio::time::sleep_until(deadline) => { status = "timed_out"; break; },
                read = output.read(&mut stdout, &mut stderr, spec.max_output_bytes) => {
                    if let Err(problem) = read { error = Some(problem); break; }
                },
                _ = tokio::time::sleep(Duration::from_millis(10)) => {},
            }
        }
    }
    let mut cleanup_errors = Vec::new();
    if let Err(problem) = custody.signal(libc_signal(false), &mut child).await {
        cleanup_errors.push(problem);
    }
    let grace = Instant::now() + GRACE;
    while Instant::now() < grace {
        match observe(&mut child, &mut custody, &mut known_exit) {
            Ok(true) if output.complete() => break,
            Ok(_) => {}
            Err(problem) => {
                cleanup_errors.push(problem);
                break;
            }
        }
        tokio::select! {
            read = output.read(&mut stdout, &mut stderr, spec.max_output_bytes) => {
                if let Err(problem) = read { error.get_or_insert(problem); }
            },
            _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        }
    }
    // The leader stays unreaped until the final group signal. Its PID/PGID
    // cannot be recycled, including when a descendant holds an output pipe.
    if let Err(problem) = custody.signal(libc_signal(true), &mut child).await {
        cleanup_errors.push(problem);
    }
    custody.disarm();
    let waited = match known_exit {
        Some(exit) => Some(exit),
        None => match timeout(SETTLE, child.wait()).await {
            Ok(Ok(exit)) => Some(exit),
            Ok(Err(problem)) => {
                cleanup_errors.push(format!("reap exec child: {problem}"));
                None
            }
            Err(_) => {
                cleanup_errors.push("exec child did not settle after kill".into());
                None
            }
        },
    };
    let drained = timeout(SETTLE, async {
        while !output.complete() {
            if let Err(problem) = output
                .read(&mut stdout, &mut stderr, spec.max_output_bytes)
                .await
            {
                error.get_or_insert(problem);
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if drained.is_err() {
        cleanup_errors.push("exec output pipes did not settle after kill".into());
    }
    if status == "completed" {
        status = if waited.is_some_and(|exit| exit.success()) {
            "succeeded"
        } else {
            "failed"
        };
    }
    let cleanup_error = (!cleanup_errors.is_empty()).then(|| cleanup_errors.join("; "));
    if let Some(problem) = &cleanup_error {
        error.get_or_insert(problem.clone());
    }
    if error.is_some() {
        status = "failed";
    }
    Outcome {
        value: result(status, waited, &output, error),
        cleanup_error,
    }
}

fn libc_signal(kill: bool) -> i32 {
    #[cfg(unix)]
    {
        if kill { libc::SIGKILL } else { libc::SIGTERM }
    }
    #[cfg(not(unix))]
    {
        i32::from(kill)
    }
}

struct Custody {
    pid: u32,
    armed: bool,
}

impl Custody {
    fn new(pid: u32) -> Self {
        Self { pid, armed: true }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
    async fn signal(&mut self, signal: i32, child: &mut Child) -> Result<(), String> {
        let deadline = Instant::now() + GRACE;
        loop {
            match self.signal_once(signal, child) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if darwin_group_may_be_settling(self.pid, &error) && Instant::now() < deadline {
                        // Terminated descendants can remain briefly visible until
                        // their new parent reaps them. Retain leader custody and
                        // retry within the existing cleanup budget; never ignore
                        // a still-populated group or a real permission failure.
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                    return Err(format!(
                        "signal exec group {} with {signal}: {error}",
                        self.pid
                    ));
                }
            }
        }
    }

    fn signal_once(&mut self, signal: i32, child: &mut Child) -> std::io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let _ = child;
            if unsafe { libc::kill(-(self.pid as i32), signal) } == -1 {
                let error = std::io::Error::last_os_error();
                #[cfg(target_os = "macos")]
                if error.raw_os_error() == Some(libc::EPERM) && empty_darwin_group(self.pid) {
                    return Ok(());
                }
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = signal;
            child.start_kill()
        }
    }
}

fn darwin_group_may_be_settling(pid: u32, error: &std::io::Error) -> bool {
    #[cfg(target_os = "macos")]
    {
        error.raw_os_error() == Some(libc::EPERM) && matches!(exited_unreaped(pid), Ok(true))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, error);
        false
    }
}

#[cfg(target_os = "macos")]
fn empty_darwin_group(pid: u32) -> bool {
    // Darwin reports EPERM for a group containing only our unreaped zombie.
    // Prove that the leader exited and libproc sees no other group members rather
    // than treating a real permission denial as successful cleanup. libproc
    // maps syscall failure to zero too, so errno must be checked separately.
    if !matches!(exited_unreaped(pid), Ok(true)) {
        return false;
    }
    let mut members: [libc::pid_t; 2] = [0; 2];
    unsafe {
        *libc::__error() = 0;
        let count = libc::proc_listpgrppids(
            pid as libc::pid_t,
            members.as_mut_ptr().cast(),
            std::mem::size_of_val(&members) as i32,
        );
        *libc::__error() == 0 && (count == 0 || (count == 1 && members[0] == pid as libc::pid_t))
    }
}

impl Drop for Custody {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.armed && exited_unreaped(self.pid).is_ok() {
            // This fallback is only for unwinding/owner cancellation. The normal
            // path signals the group, disarms, and then explicitly reaps.
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
            }
        }
    }
}

fn observe(
    child: &mut Child,
    custody: &mut Custody,
    known_exit: &mut Option<ExitStatus>,
) -> Result<bool, String> {
    #[cfg(unix)]
    {
        let _ = (child, known_exit);
        match exited_unreaped(custody.pid) {
            Ok(exited) => Ok(exited),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => Ok(false),
            Err(error) => {
                if error.raw_os_error() == Some(libc::ECHILD) {
                    custody.disarm();
                }
                Err(format!("observe exec child without reaping: {error}"))
            }
        }
    }
    #[cfg(not(unix))]
    {
        match child
            .try_wait()
            .map_err(|e| format!("observe exec child: {e}"))?
        {
            Some(exit) => {
                custody.disarm();
                *known_exit = Some(exit);
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(unix)]
fn exited_unreaped(pid: u32) -> std::io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: writable storage and our direct child. WNOWAIT retains custody of
    // its PID until all owned-group signals have been issued.
    let status = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if status != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init().si_pid() } != 0)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, thread};
    use tempfile::TempDir;
    use tokio::runtime::{Builder, Runtime};

    fn setup() -> (Runtime, TempDir, Supervisor) {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let supervisor =
            Supervisor::new(runtime.handle().clone(), directory.path(), directory.path()).unwrap();
        (runtime, directory, supervisor)
    }

    fn shell(supervisor: &mut Supervisor, body: &str, extra: Value) -> u64 {
        let mut spec = json!({"program":"/bin/sh","args":["-c",body]});
        spec.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        supervisor.start(&spec.to_string()).unwrap()
    }

    fn settle(supervisor: &mut Supervisor, id: u64) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = supervisor.poll(id).unwrap() {
                return serde_json::from_str(&value).unwrap();
            }
            assert!(std::time::Instant::now() < deadline, "exec did not settle");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_file(path: &Path) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(value) = fs::read_to_string(path)
                && !value.trim().is_empty()
            {
                return value;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture did not create {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn assert_reaped(pid: i32) {
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "direct child remains");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    // macOS rejects non-UTF-8 directory entries at mkdir; Linux permits them.
    #[cfg(target_os = "linux")]
    #[test]
    fn canonical_command_cwd_rejects_non_utf8_before_recording_or_spawning() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let (_runtime, directory, mut supervisor) = setup();
        let target = directory
            .path()
            .join(OsString::from_vec(b"non-utf8-\xff".to_vec()));
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, directory.path().join("alias")).unwrap();
        let error = supervisor
            .start(r#"{"program":"/bin/echo","cwd":"alias"}"#)
            .unwrap_err();
        assert!(error.contains("UTF-8 path"), "{error}");
        assert!(supervisor.jobs.is_empty());
        supervisor.finish().unwrap();
        assert_eq!(
            fs::metadata(directory.path().join("exec.jsonl"))
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn argv_is_literal_and_receipt_is_private() {
        let (_runtime, directory, mut supervisor) = setup();
        let id = supervisor
            .start(
                &json!({"program":"/bin/echo","args":["$(touch injected); literal"]}).to_string(),
            )
            .unwrap();
        let result = settle(&mut supervisor, id);
        assert_eq!(result["status"], "succeeded", "{result}");
        assert_eq!(result["stdout"], "$(touch injected); literal\n");
        assert!(!directory.path().join("injected").exists());
        assert!(supervisor.poll(id).unwrap_err().contains("consumed"));
        supervisor.finish().unwrap();
        let path = directory.path().join("exec.jsonl");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let records: Vec<Value> = fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1]["kind"], "exec_spawned");
        assert_reaped(records[1]["pid"].as_i64().unwrap() as i32);
        assert_eq!(records[2]["result"], result);
    }

    #[test]
    fn failed_check_and_failed_launch_have_distinct_evidence() {
        let (_runtime, _directory, mut supervisor) = setup();
        let id = shell(
            &mut supervisor,
            "printf out; printf err >&2; exit 7",
            json!({}),
        );
        let check = settle(&mut supervisor, id);
        assert_eq!(check["status"], "failed");
        assert_eq!(check["exit_code"], 7);
        assert!(check["error"].is_null());
        assert_eq!(check["stdout"], "out");
        assert_eq!(check["stderr"], "err");
        let id = supervisor
            .start(r#"{"program":"/no/such/due-verifier"}"#)
            .unwrap();
        let launch = settle(&mut supervisor, id);
        assert_eq!(launch["status"], "failed");
        assert!(launch["exit_code"].is_null());
        assert!(launch["error"].as_str().unwrap().contains("spawn exec"));
        supervisor.finish().unwrap();
    }

    #[test]
    fn cancellation_and_deadline_join_owned_processes() {
        for cancelled in [false, true] {
            let (_runtime, directory, mut supervisor) = setup();
            let id = shell(
                &mut supervisor,
                "echo $$ > leader; sleep 30",
                json!({"timeout_ms":if cancelled { 30000 } else { 100 }}),
            );
            let pid = wait_file(&directory.path().join("leader"))
                .trim()
                .parse()
                .unwrap();
            if cancelled {
                supervisor.cancel(id).unwrap();
                supervisor.cancel(id).unwrap();
            }
            let result = settle(&mut supervisor, id);
            assert_eq!(
                result["status"],
                if cancelled { "cancelled" } else { "timed_out" },
                "{result}"
            );
            assert_reaped(pid);
            supervisor.finish().unwrap();
        }
    }

    #[test]
    fn combined_output_is_bounded_and_overflow_settles() {
        let (_runtime, directory, mut supervisor) = setup();
        let id = shell(
            &mut supervisor,
            "echo $$ > leader; printf 12345678; printf abcdefgh >&2; exec yes x",
            json!({"max_output_bytes":12}),
        );
        let pid = wait_file(&directory.path().join("leader"))
            .trim()
            .parse()
            .unwrap();
        let result = settle(&mut supervisor, id);
        assert_eq!(result["status"], "failed");
        assert_eq!(
            result["stdout"].as_str().unwrap().len() + result["stderr"].as_str().unwrap().len(),
            12
        );
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("output exceeded")
        );
        assert_reaped(pid);
        supervisor.finish().unwrap();
    }

    #[test]
    fn invalid_utf8_is_an_infrastructure_error_instead_of_replacement_text() {
        let (_runtime, _directory, mut supervisor) = setup();
        let id = shell(
            &mut supervisor,
            "printf '\\377'",
            json!({"max_output_bytes":1}),
        );
        let result = settle(&mut supervisor, id);
        assert_eq!(result["status"], "failed");
        assert_eq!(result["stdout"], "");
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("not valid UTF-8")
        );
        supervisor.finish().unwrap();
    }

    #[test]
    fn parent_exit_settles_descendant_held_pipes_without_signalling_unrelated_child() {
        let (_runtime, directory, mut supervisor) = setup();
        let mut unrelated = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let id = shell(
            &mut supervisor,
            "sleep 30 & echo $! > descendant; echo $$ > leader; exit 0",
            json!({}),
        );
        let leader = wait_file(&directory.path().join("leader"))
            .trim()
            .parse()
            .unwrap();
        let descendant: i32 = wait_file(&directory.path().join("descendant"))
            .trim()
            .parse()
            .unwrap();
        let result = settle(&mut supervisor, id);
        assert_eq!(result["status"], "succeeded", "{result}");
        assert_reaped(leader);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let gone = unsafe { libc::kill(descendant, 0) } == -1;
            // An orphan zombie is settled but belongs to the host reaper, not Due.
            let zombie = fs::read_to_string(format!("/proc/{descendant}/stat")).is_ok_and(|s| {
                s.split_once(") ")
                    .is_some_and(|(_, tail)| tail.starts_with('Z'))
            });
            if gone || zombie {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owned descendant is running"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        supervisor.finish().unwrap();
    }

    #[test]
    fn scope_exit_rejects_abandoned_operations_after_joining_them() {
        let (_runtime, directory, mut supervisor) = setup();
        shell(&mut supervisor, "echo $$ > leader; sleep 30", json!({}));
        let pid = wait_file(&directory.path().join("leader"))
            .trim()
            .parse()
            .unwrap();
        let error = supervisor.finish().unwrap_err();
        assert!(error.contains("unconsumed"));
        assert_reaped(pid);
        assert_eq!(supervisor.finish().unwrap_err(), error);
        assert!(
            supervisor
                .start(r#"{"program":"/bin/true"}"#)
                .unwrap_err()
                .contains("closed")
        );
    }

    #[test]
    fn validation_and_pending_capacity_do_not_launch_extra_work() {
        let (_runtime, _directory, mut supervisor) = setup();
        for invalid in [
            json!({}),
            json!({"program":""}),
            json!({"program":"/bin/true","shell":true}),
            json!({"program":"/bin/true","timeout_ms":0}),
            json!({"program":"/bin/true","timeout_ms":300001}),
            json!({"program":"/bin/true","max_output_bytes":1048577}),
            json!({"program":"/bin/true","args":[1]}),
            json!({"program":"/bin/true","cwd":null}),
        ] {
            assert!(supervisor.start(&invalid.to_string()).is_err(), "{invalid}");
        }
        let ids: Vec<_> = (0..8)
            .map(|_| shell(&mut supervisor, "sleep 30", json!({})))
            .collect();
        assert!(
            supervisor
                .start(r#"{"program":"/bin/true"}"#)
                .unwrap_err()
                .contains("8 pending")
        );
        for id in &ids {
            supervisor.cancel(*id).unwrap();
        }
        for id in ids {
            assert_eq!(settle(&mut supervisor, id)["status"], "cancelled");
        }
        let id = supervisor.start(r#"{"program":"true"}"#).unwrap();
        assert_eq!(settle(&mut supervisor, id)["status"], "succeeded");
        supervisor.finish().unwrap();
    }

    #[test]
    fn receipt_limit_rejects_before_spawning_and_remains_sticky() {
        let (_runtime, directory, mut supervisor) = setup();
        supervisor.journal.lock().unwrap().limit = 1;
        let spec = json!({"program":"/bin/sh","args":["-c","touch should-not-exist"]});
        let error = supervisor.start(&spec.to_string()).unwrap_err();
        assert!(error.contains("receipt exceeded"), "{error}");
        assert_eq!(supervisor.start(&spec.to_string()).unwrap_err(), error);
        assert!(supervisor.jobs.is_empty());
        assert!(!directory.path().join("should-not-exist").exists());
        assert_eq!(
            fs::metadata(directory.path().join("exec.jsonl"))
                .unwrap()
                .len(),
            0
        );
        assert!(supervisor.finish().unwrap_err().contains(&error));
    }

    #[test]
    fn completion_receipt_failure_survives_result_consumption() {
        let (_runtime, directory, mut supervisor) = setup();
        let id = shell(
            &mut supervisor,
            "echo $$ > leader; while [ ! -f release ]; do sleep 0.01; done; printf checked",
            json!({}),
        );
        let pid: i32 = wait_file(&directory.path().join("leader"))
            .trim()
            .parse()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let receipt = fs::read_to_string(directory.path().join("exec.jsonl")).unwrap();
            if receipt.contains("exec_spawned") {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        let bytes = {
            let mut journal = supervisor.journal.lock().unwrap();
            journal.limit = journal.bytes;
            journal.bytes
        };
        fs::write(directory.path().join("release"), "go").unwrap();
        let value = settle(&mut supervisor, id);
        assert_eq!(value["stdout"], "checked", "{value}");
        assert_eq!(value["status"], "failed", "{value}");
        assert!(
            value["error"]
                .as_str()
                .unwrap()
                .contains("receipt exceeded")
        );
        assert_reaped(pid);
        assert_eq!(
            fs::metadata(directory.path().join("exec.jsonl"))
                .unwrap()
                .len(),
            bytes as u64
        );
        assert!(
            supervisor
                .start(r#"{"program":"true"}"#)
                .unwrap_err()
                .contains("receipt exceeded")
        );
        assert!(
            supervisor
                .finish()
                .unwrap_err()
                .contains("receipt exceeded")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_cancelled_shell_descendants_settle_without_false_permission_failure() {
        // Exercise the scheduling window where SIGTERM has exited both the
        // shell and its descendant, but Darwin still lists both group members.
        for _ in 0..16 {
            let (_runtime, directory, mut supervisor) = setup();
            let id = shell(
                &mut supervisor,
                "sleep 30 & echo $! > descendant; echo $$ > leader; wait",
                json!({}),
            );
            let leader = wait_file(&directory.path().join("leader"))
                .trim()
                .parse()
                .unwrap();
            let descendant = wait_file(&directory.path().join("descendant"))
                .trim()
                .parse()
                .unwrap();
            supervisor.cancel(id).unwrap();
            let result = settle(&mut supervisor, id);
            assert_eq!(result["status"], "cancelled", "{result}");
            assert!(result["error"].is_null(), "{result}");
            assert_reaped(leader);
            assert_reaped(descendant);
            supervisor.finish().unwrap();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_empty_group_requires_the_exited_owned_leader() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(!empty_darwin_group(pid));
        child.kill().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !exited_unreaped(pid).unwrap() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        assert!(empty_darwin_group(pid));
        child.wait().unwrap();
        assert!(!empty_darwin_group(pid));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "manual causal pipe-window probe; see target/issue19"]
    fn issue19_current_runtime_sibling_pipe_probe() {
        let root = PathBuf::from(std::env::var_os("DUE19_EVIDENCE_DIR").unwrap());
        let program = PathBuf::from(std::env::var_os("DUE19_CHILD").unwrap());
        let injected = std::env::var("DUE19_INJECT").as_deref() == Ok("1");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("parent.pid"), format!("{}\n", std::process::id())).unwrap();
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let mut supervisor = Supervisor::new(runtime.handle().clone(), &root, &root).unwrap();
        let spec = |role: &str| {
            json!({
                "program":program,"args":[root,role],"timeout_ms":15000
            })
            .to_string()
        };
        let a = supervisor.start(&spec("a")).unwrap();
        let a_pid = wait_file(&root.join("a.ready")).trim().parse().unwrap();
        if injected {
            fs::write(root.join("arm"), "arm").unwrap();
        }
        let ready = supervisor.start(&spec("r")).unwrap();
        if injected {
            let window: Value =
                serde_json::from_str(&wait_file(&root.join("window.json"))).unwrap();
            assert_eq!(window["write_flags"], 0, "injection must precede CLOEXEC");
        } else {
            wait_file(&root.join("r.ready"));
        }
        let b = supervisor.start(&spec("b")).unwrap();
        let b_pid: i32 = wait_file(&root.join("b.ready")).trim().parse().unwrap();
        let ready_result = settle(&mut supervisor, ready);
        // Capture evidence and join A while B is still an active sibling.
        supervisor.cancel(a).unwrap();
        let a_result = settle(&mut supervisor, a);
        assert_reaped(a_pid);
        fs::write(root.join("challenge"), "fresh challenge").unwrap();
        let challenge = wait_file(&root.join("b.ack"));
        assert_eq!(unsafe { libc::kill(b_pid, 0) }, 0);
        assert!(!root.join("release").exists());
        let before_release = fs::read_to_string(root.join("exec.jsonl")).unwrap();
        fs::write(root.join("before-release.jsonl"), before_release).unwrap();
        fs::write(root.join("release"), "release").unwrap();
        let b_result = settle(&mut supervisor, b);
        assert_reaped(b_pid);
        let finish = supervisor.finish();
        let mut inherited_writer = false;
        if injected {
            let window: Value =
                serde_json::from_str(&fs::read_to_string(root.join("window.json")).unwrap())
                    .unwrap();
            let readiness: Vec<Value> = fs::read_to_string(root.join("r.fds.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(
                readiness
                    .iter()
                    .any(|fd| fd["fd"] == 1 && fd["handle"] == window["write_handle"]),
                "probe must identify readiness stdout"
            );
            let siblings: Vec<Value> = fs::read_to_string(root.join("b.fds.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            inherited_writer = siblings
                .iter()
                .any(|fd| fd["handle"] == window["write_handle"] && fd["access"] == libc::O_WRONLY);
        }
        let report = json!({"injected":injected,"ready":ready_result,"a":a_result,
            "b":b_result,"challenge":challenge,"finish_error":finish.err(),
            "same_writer_in_sibling":inherited_writer});
        fs::write(root.join("report.json"), format!("{report:#}\n")).unwrap();
        println!("{report:#}");
        assert_eq!(report["a"]["status"], "cancelled", "{report}");
        assert_eq!(report["b"]["status"], "succeeded", "{report}");
        assert_eq!(report["ready"]["status"], "succeeded", "{report}");
        assert!(report["finish_error"].is_null(), "{report}");
        assert_eq!(report["same_writer_in_sibling"], false, "{report}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_sibling_pipe_collection_survives_the_cloexec_window() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let library = root.join("pipe-window.dylib");
        let child = root.join("child");
        for (source, output, dynamic) in [
            ("issue19-pipe-window.c", &library, true),
            ("issue19-child.c", &child, false),
        ] {
            let mut compiler = std::process::Command::new("/usr/bin/clang");
            compiler.args(["-std=c11", "-Wall", "-Wextra", "-Werror"]);
            if dynamic {
                compiler.arg("-dynamiclib");
            }
            let compiled = compiler
                .arg(fixtures.join(source))
                .arg("-o")
                .arg(output)
                .output()
                .unwrap();
            assert!(
                compiled.status.success(),
                "{}",
                String::from_utf8_lossy(&compiled.stderr)
            );
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "supervisor::tests::issue19_current_runtime_sibling_pipe_probe",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("DUE19_EVIDENCE_DIR", root)
            .env("DUE19_CHILD", &child)
            .env("DUE19_INJECT", "1")
            .env("DYLD_INSERT_LIBRARIES", &library)
            .stdout(File::create(root.join("probe.stdout")).unwrap())
            .stderr(File::create(root.join("probe.stderr")).unwrap())
            .status()
            .unwrap();
        if !output.success() {
            let preserved = directory.keep();
            panic!(
                "causal pipe-window probe failed; evidence: {}",
                preserved.display()
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cancellation_before_spawn_boundary_release_skips_the_child() {
        let (_runtime, directory, mut supervisor) = setup();
        let guard = crate::PROCESS_SPAWN.lock().unwrap();
        let id = shell(&mut supervisor, "echo unexpected > launched", json!({}));
        supervisor.cancel(id).unwrap();
        drop(guard);
        let outcome = settle(&mut supervisor, id);
        assert_eq!(outcome["status"], "cancelled", "{outcome}");
        assert!(!directory.path().join("launched").exists());
        let receipt = fs::read_to_string(directory.path().join("exec.jsonl")).unwrap();
        assert!(!receipt.contains("exec_spawned"), "{receipt}");
        supervisor.finish().unwrap();
    }
}
