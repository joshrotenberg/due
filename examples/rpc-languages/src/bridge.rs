//! One owned Due child and a bounded, language-neutral raw-frame transport.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::{Builder, Runtime},
    sync::{mpsc, watch},
    task::JoinHandle,
    time::timeout,
};

const MAX_FRAME: usize = 8 * 1024 * 1024;
const MAX_REQUEST: usize = 1024 * 1024;
const MAX_CAPTURE: usize = 64 * 1024 * 1024;
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUEUE_FRAMES: usize = 256;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

struct State {
    failure: watch::Sender<Option<String>>,
    queued_bytes: AtomicUsize,
    captured_bytes: AtomicUsize,
    draining: AtomicBool,
}

impl State {
    fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        self.failure.send_if_modified(|current| {
            if current.is_some() {
                false
            } else {
                *current = Some(message);
                true
            }
        });
    }

    fn error(&self) -> Option<String> {
        self.failure.borrow().clone()
    }
}

/// Synchronous embedding facade; the reader continues while guest code runs.
pub struct Bridge {
    runtime: Runtime,
    child: Child,
    input: Option<ChildStdin>,
    frames: mpsc::Receiver<String>,
    reader: Option<JoinHandle<()>>,
    signals: Option<JoinHandle<()>>,
    state: Arc<State>,
    evidence: PathBuf,
    pid: u32,
    finished: Option<Result<(), String>>,
}

impl Bridge {
    /// Launch an absolute Due binary with `rpc`, using a new evidence directory.
    pub fn spawn(program: &Path, cwd: &Path, evidence: &Path) -> Result<Self, String> {
        if !program.is_absolute() {
            return Err("Due binary path must be absolute".into());
        }
        fs::create_dir(evidence).map_err(|e| format!("create evidence directory: {e}"))?;
        let capture = private_file(&evidence.join("stdout.bin"))?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("create bridge runtime: {e}"))?;
        let (failure, _) = watch::channel(None);
        let state = Arc::new(State {
            failure,
            queued_bytes: AtomicUsize::new(0),
            captured_bytes: AtomicUsize::new(0),
            draining: AtomicBool::new(false),
        });
        let (mut child, signal_future) = {
            let _entered = runtime.enter();
            // Install cancellation listeners before launching an owned child.
            let signal_future = listen_for_signal(Arc::clone(&state))?;
            let child = Command::new(program)
                .arg("rpc")
                .current_dir(cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("spawn Due RPC: {e}"))?;
            (child, signal_future)
        };
        let pid = child.id().ok_or("spawned child has no PID")?;
        let input = child.stdin.take();
        let stdout = child.stdout.take().ok_or("spawned child has no stdout")?;
        let (sender, frames) = mpsc::channel(MAX_QUEUE_FRAMES);
        let reader_state = Arc::clone(&state);
        let reader = runtime.spawn(read_stdout(stdout, capture, sender, reader_state));
        let signals = runtime.spawn(signal_future);
        Ok(Self {
            runtime,
            child,
            input,
            frames,
            reader: Some(reader),
            signals: Some(signals),
            state,
            evidence: evidence.to_owned(),
            pid,
            finished: None,
        })
    }

    /// The first transport failure remains authoritative even if a guest catches it.
    pub fn error(&self) -> Option<String> {
        self.state.error()
    }

    pub fn check(&self) -> Result<(), String> {
        self.error().map_or(Ok(()), Err)
    }

    /// Send one raw UTF-8 JSON line. The bridge deliberately does not parse it.
    pub fn send(&mut self, line: &str) -> Result<(), String> {
        self.check()?;
        if self.finished.is_some() {
            return Err("bridge is closed".into());
        }
        if line.contains(['\r', '\n']) || line.len() > MAX_REQUEST {
            self.state.fail("request must be one line of at most 1 MiB");
            return self.check();
        }
        let Some(input) = self.input.as_mut() else {
            return Err("bridge input is closed".into());
        };
        let result = self.runtime.block_on(async {
            timeout(WRITE_TIMEOUT, async {
                input.write_all(line.as_bytes()).await?;
                input.write_all(b"\n").await?;
                input.flush().await
            })
            .await
        });
        match result {
            Ok(Ok(())) => self.check(),
            Ok(Err(error)) => {
                self.state.fail(format!("write RPC request: {error}"));
                self.check()
            }
            Err(_) => {
                self.state.fail("RPC request write timed out");
                self.check()
            }
        }
    }

    /// Return a frame, or `None` on timeout. EOF is never confused with timeout.
    pub fn recv(&mut self, timeout_ms: u64) -> Result<Option<String>, String> {
        self.check()?;
        if self.finished.is_some() {
            return Err("bridge is closed".into());
        }
        if timeout_ms > 10_000 {
            return Err("receive timeout must be at most 10000 ms".into());
        }
        let mut failure = self.state.failure.subscribe();
        let result = self.runtime.block_on(async {
            // Subscribe before checking so a reader failure cannot miss the wake.
            if failure.borrow().is_some() {
                return None;
            }
            tokio::select! {
                biased;
                _ = failure.changed() => None,
                frame = self.frames.recv() => Some(frame),
                _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => None,
            }
        });
        // Account dequeued bytes even when failure races with delivery.
        if let Some(Some(ref frame)) = result {
            self.state
                .queued_bytes
                .fetch_sub(frame.len(), Ordering::AcqRel);
        }
        self.check()?;
        match result {
            Some(Some(frame)) => Ok(Some(frame)),
            None => Ok(None),
            Some(None) => {
                self.state.fail("RPC stdout reached EOF");
                self.check().map(|()| None)
            }
        }
    }

    /// Close stdin, settle the child and reader, and save an exit receipt.
    ///
    /// Remaining frames are captured but no longer delivered to the guest.
    /// A forced kill is recorded as a failure, even if reaping succeeds.
    pub fn finish(&mut self) -> Result<(), String> {
        if let Some(result) = &self.finished {
            return result.clone();
        }
        self.state.draining.store(true, Ordering::Release);
        self.input.take();
        let mut forced_kill = false;
        let mut status = None;
        self.runtime.block_on(async {
            match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
                Ok(Ok(exit)) => status = Some(exit),
                Ok(Err(error)) => self.state.fail(format!("wait for Due child: {error}")),
                Err(_) => {
                    self.state
                        .fail("Due child did not exit after stdin closed; forced kill");
                }
            }
            // Also attempt settlement after an unexpected wait error.
            if status.is_none() {
                forced_kill = true;
                if let Err(error) = self.child.start_kill() {
                    self.state.fail(format!("kill Due child: {error}"));
                }
                match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
                    Ok(Ok(exit)) => status = Some(exit),
                    Ok(Err(error)) => self.state.fail(format!("reap Due child: {error}")),
                    Err(_) => self.state.fail("Due child did not settle after kill"),
                }
            }
            if let Some(mut reader) = self.reader.take() {
                match timeout(SHUTDOWN_TIMEOUT, &mut reader).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => self
                        .state
                        .fail(format!("stdout reader task failed: {error}")),
                    Err(_) => {
                        self.state
                            .fail("stdout did not close after Due child settlement");
                        reader.abort();
                        let _ = reader.await;
                    }
                }
            }
            if let Some(signals) = self.signals.take() {
                signals.abort();
                let _ = signals.await;
            }
        });
        if let Some(exit) = status
            && !exit.success()
        {
            self.state
                .fail(format!("Due child exited unsuccessfully: {exit}"));
        }
        let receipt = serde_json::json!({
            "pid": self.pid,
            "exit_code": status.and_then(|s| s.code()),
            "exit_signal": status.and_then(exit_signal),
            "settled": status.is_some(),
            "forced_kill": forced_kill,
            "transport_error": self.error(),
            "raw_bytes": fs::metadata(self.evidence.join("stdout.bin")).map(|m| m.len()).unwrap_or(self.state.captured_bytes.load(Ordering::Acquire) as u64),
        });
        if let Err(error) = private_file(&self.evidence.join("receipt.json"))
            .and_then(|mut file| writeln!(file, "{receipt}").map_err(|e| e.to_string()))
        {
            self.state.fail(format!("write bridge receipt: {error}"));
        }
        let result = self.check();
        self.finished = Some(result.clone());
        result
    }
}

#[cfg(unix)]
fn listen_for_signal(
    state: Arc<State>,
) -> Result<impl Future<Output = ()> + Send + 'static, String> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt =
        signal(SignalKind::interrupt()).map_err(|e| format!("listen for SIGINT: {e}"))?;
    let mut terminate =
        signal(SignalKind::terminate()).map_err(|e| format!("listen for SIGTERM: {e}"))?;
    let mut hangup = signal(SignalKind::hangup()).map_err(|e| format!("listen for SIGHUP: {e}"))?;
    Ok(async move {
        let signal = tokio::select! {
            _ = interrupt.recv() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
            _ = hangup.recv() => "SIGHUP",
        };
        state.fail(format!("runner received {signal}"));
    })
}

#[cfg(not(unix))]
fn listen_for_signal(
    state: Arc<State>,
) -> Result<impl Future<Output = ()> + Send + 'static, String> {
    Ok(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => state.fail("runner received interrupt"),
            Err(error) => state.fail(format!("listen for interrupt: {error}")),
        }
    })
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn private_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))
}

#[cfg(unix)]
fn exit_signal(status: ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_: ExitStatus) -> Option<i32> {
    None
}

async fn read_stdout(
    mut stdout: ChildStdout,
    mut capture: File,
    sender: mpsc::Sender<String>,
    state: Arc<State>,
) {
    let mut chunk = [0_u8; 8192];
    let mut frame = Vec::new();
    loop {
        let count = match stdout.read(&mut chunk).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                state.fail(format!("read RPC stdout: {error}"));
                break;
            }
        };
        let remaining = MAX_CAPTURE - state.captured_bytes.load(Ordering::Acquire);
        let retained = remaining.min(count);
        // Preserve the exact bounded prefix before framing, UTF-8 or guest JSON parsing.
        if let Err(error) = capture.write_all(&chunk[..retained]) {
            state.fail(format!("write stdout evidence: {error}"));
            break;
        }
        state.captured_bytes.fetch_add(retained, Ordering::Release);
        if retained < count {
            state.fail("stdout evidence exceeded 64 MiB");
        }
        if state.error().is_some() {
            frame.clear();
            continue;
        }
        for bytes in chunk[..count].split_inclusive(|byte| *byte == b'\n') {
            let newline = bytes.last() == Some(&b'\n');
            let contents = if newline {
                &bytes[..bytes.len() - 1]
            } else {
                bytes
            };
            if frame.len() + contents.len() > MAX_FRAME {
                state.fail("RPC stdout frame exceeded 8 MiB");
                frame.clear();
                break;
            }
            frame.extend_from_slice(contents);
            if newline {
                let line = match String::from_utf8(std::mem::take(&mut frame)) {
                    Ok(line) => line,
                    Err(_) => {
                        state.fail("RPC stdout contains malformed UTF-8");
                        break;
                    }
                };
                if state.draining.load(Ordering::Acquire) {
                    continue;
                }
                let size = line.len();
                let previous = state.queued_bytes.fetch_add(size, Ordering::AcqRel);
                if previous + size > MAX_QUEUE_BYTES {
                    state.queued_bytes.fetch_sub(size, Ordering::AcqRel);
                    state.fail("RPC frame queue exceeded 8 MiB");
                    break;
                }
                if sender.try_send(line).is_err() {
                    state.queued_bytes.fetch_sub(size, Ordering::AcqRel);
                    state.fail("RPC frame queue exceeded 256 frames or receiver closed");
                    break;
                }
            }
        }
    }
    if !frame.is_empty() {
        state.fail("RPC stdout ended with an unterminated frame");
    }
    if let Err(error) = capture.flush() {
        state.fail(format!("flush stdout evidence: {error}"));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use tempfile::TempDir;

    struct Fixture {
        dir: TempDir,
        binary: PathBuf,
        evidence: PathBuf,
    }

    impl Fixture {
        fn new(body: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("peer");
            fs::write(&binary, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            let evidence = dir.path().join("evidence");
            Self {
                dir,
                binary,
                evidence,
            }
        }

        fn spawn(&self) -> Bridge {
            Bridge::spawn(&self.binary, self.dir.path(), &self.evidence).unwrap()
        }

        fn raw(&self) -> Vec<u8> {
            fs::read(self.evidence.join("stdout.bin")).unwrap()
        }

        fn receipt(&self) -> serde_json::Value {
            serde_json::from_slice(&fs::read(self.evidence.join("receipt.json")).unwrap()).unwrap()
        }
    }

    #[test]
    fn round_trip_and_clean_close_preserve_raw_bytes() {
        let fixture = Fixture::new("while IFS= read -r line; do printf '%s\\n' \"$line\"; done");
        let mut bridge = fixture.spawn();
        let line = r#"{"large":9007199254740993,"nil":null,"array":[],"object":{}}"#;
        bridge.send(line).unwrap();
        assert_eq!(bridge.recv(1000).unwrap().as_deref(), Some(line));
        bridge.finish().unwrap();
        bridge.finish().unwrap();
        assert_eq!(fixture.raw(), format!("{line}\n").as_bytes());
        assert_eq!(fixture.receipt()["exit_code"], 0);
        assert_eq!(fixture.receipt()["settled"], true);
        assert_eq!(fixture.receipt()["forced_kill"], false);
        assert!(fixture.receipt()["transport_error"].is_null());
        assert_eq!(
            fs::metadata(fixture.evidence.join("stdout.bin"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn receive_timeout_is_distinct_from_eof() {
        let fixture = Fixture::new("while IFS= read -r line; do :; done");
        let mut bridge = fixture.spawn();
        assert_eq!(bridge.recv(10).unwrap(), None);
        assert!(bridge.recv(10_001).unwrap_err().contains("10000"));
        bridge.finish().unwrap();
        assert!(bridge.recv(0).unwrap_err().contains("closed"));

        let fixture = Fixture::new("exit 0");
        let mut bridge = fixture.spawn();
        assert!(bridge.recv(1000).unwrap_err().contains("EOF"));
        assert!(bridge.finish().unwrap_err().contains("EOF"));
        assert_eq!(fixture.receipt()["exit_code"], 0);
    }

    #[test]
    fn split_utf8_is_assembled_without_replacement() {
        let fixture = Fixture::new(
            r"printf '\360\235'; sleep 0.05; printf '\204\236\n'; while IFS= read -r line; do :; done",
        );
        let mut bridge = fixture.spawn();
        assert_eq!(bridge.recv(1000).unwrap().as_deref(), Some("\u{1d11e}"));
        bridge.finish().unwrap();
        assert_eq!(fixture.raw(), "\u{1d11e}\n".as_bytes());
    }

    #[test]
    fn malformed_utf8_is_sticky_and_raw_evidence_survives() {
        let fixture = Fixture::new(r"printf '\377\n'; while IFS= read -r line; do :; done");
        let mut bridge = fixture.spawn();
        let error = bridge.recv(1000).unwrap_err();
        assert!(error.contains("malformed UTF-8"), "{error}");
        assert_eq!(bridge.send("{}"), Err(error.clone()));
        assert_eq!(bridge.finish(), Err(error));
        assert_eq!(fixture.raw(), [255, 10]);
        assert_eq!(fixture.receipt()["settled"], true);
    }

    #[test]
    fn unterminated_frame_is_not_returned_as_complete() {
        let fixture = Fixture::new("printf '{\"ok\":true}'");
        let mut bridge = fixture.spawn();
        assert!(bridge.recv(1000).unwrap_err().contains("unterminated"));
        assert!(bridge.finish().is_err());
        assert_eq!(fixture.raw(), br#"{"ok":true}"#);
    }

    #[test]
    fn stalled_guest_fails_on_queue_overflow_without_silent_loss() {
        let fixture = Fixture::new(
            "i=0; while [ \"$i\" -lt 257 ]; do printf 'x\\n'; i=$((i + 1)); done; while IFS= read -r line; do :; done",
        );
        let mut bridge = fixture.spawn();
        for _ in 0..100 {
            if bridge.error().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(bridge.error().unwrap().contains("256 frames"));
        assert!(bridge.finish().is_err());
        assert_eq!(fixture.raw().len(), 514);
        assert_eq!(fixture.receipt()["raw_bytes"], 514);
    }

    #[test]
    fn oversized_frame_is_bounded_and_capture_continues() {
        let fixture = Fixture::new("dd if=/dev/zero bs=1024 count=8193 2>/dev/null");
        let mut bridge = fixture.spawn();
        assert!(
            bridge
                .recv(1000)
                .unwrap_err()
                .contains("frame exceeded 8 MiB")
        );
        assert!(bridge.finish().is_err());
        assert_eq!(fixture.raw().len(), 8193 * 1024);
        assert_eq!(fixture.receipt()["settled"], true);
    }

    #[test]
    fn queued_bytes_are_bounded_independently_of_frame_count() {
        let fixture = Fixture::new(
            "i=0; while [ \"$i\" -lt 9 ]; do dd if=/dev/zero bs=1024 count=1024 2>/dev/null; printf '\\n'; i=$((i + 1)); done; while IFS= read -r line; do :; done",
        );
        let mut bridge = fixture.spawn();
        for _ in 0..200 {
            if bridge.error().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(bridge.error().unwrap().contains("queue exceeded 8 MiB"));
        assert!(bridge.finish().is_err());
        assert_eq!(fixture.raw().len(), 9 * (1024 * 1024 + 1));
    }

    #[test]
    fn capture_keeps_exact_bounded_prefix_even_during_shutdown() {
        let fixture = Fixture::new(
            "while IFS= read -r line; do :; done; i=0; while [ \"$i\" -lt 65 ]; do dd if=/dev/zero bs=1024 count=1024 2>/dev/null; printf '\\n'; i=$((i + 1)); done",
        );
        let mut bridge = fixture.spawn();
        assert!(
            bridge
                .finish()
                .unwrap_err()
                .contains("evidence exceeded 64 MiB")
        );
        let raw = fixture.raw();
        assert_eq!(raw.len(), MAX_CAPTURE);
        assert_eq!(raw[1024 * 1024], b'\n');
        assert!(raw[..1024 * 1024].iter().all(|byte| *byte == 0));
        assert_eq!(fixture.receipt()["raw_bytes"], MAX_CAPTURE);
        assert_eq!(fixture.receipt()["settled"], true);
        assert_eq!(fixture.receipt()["forced_kill"], false);
    }

    #[test]
    fn request_limits_fail_before_writing_and_are_sticky() {
        let fixture = Fixture::new("while IFS= read -r line; do printf '%s\\n' \"$line\"; done");
        let mut bridge = fixture.spawn();
        let error = bridge.send("{}\n{}").unwrap_err();
        assert!(error.contains("one line"));
        assert_eq!(bridge.send("{}"), Err(error));
        assert!(bridge.finish().is_err());
        assert!(fixture.raw().is_empty());
    }

    #[test]
    fn drop_settles_child_and_writes_receipt() {
        let fixture = Fixture::new("while IFS= read -r line; do :; done");
        drop(fixture.spawn());
        assert_eq!(fixture.receipt()["exit_code"], 0);
        assert_eq!(fixture.receipt()["settled"], true);
    }

    #[test]
    fn nonzero_exit_is_not_successful_cleanup() {
        let fixture = Fixture::new("while IFS= read -r line; do :; done; exit 7");
        let mut bridge = fixture.spawn();
        assert!(bridge.finish().unwrap_err().contains("unsuccessfully"));
        assert_eq!(fixture.receipt()["exit_code"], 7);
    }

    #[test]
    fn refusing_eof_is_killed_and_reaped_with_failure_receipt() {
        let fixture = Fixture::new("while :; do :; done");
        let mut bridge = fixture.spawn();
        assert!(bridge.finish().unwrap_err().contains("forced kill"));
        assert_eq!(fixture.receipt()["forced_kill"], true);
        assert_eq!(fixture.receipt()["settled"], true);
        assert_eq!(fixture.receipt()["exit_signal"], 9);
    }

    #[test]
    fn evidence_collision_does_not_overwrite_existing_capture() {
        let fixture = Fixture::new("exit 0");
        fs::create_dir(&fixture.evidence).unwrap();
        fs::write(fixture.evidence.join("stdout.bin"), b"original").unwrap();
        assert!(Bridge::spawn(&fixture.binary, fixture.dir.path(), &fixture.evidence).is_err());
        assert_eq!(fixture.raw(), b"original");
    }
}
