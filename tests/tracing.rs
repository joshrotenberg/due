#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    binary: PathBuf,
    provider: &'static str,
}

impl Fixture {
    fn new(provider: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("streaming-provider");
        fs::write(&binary, include_bytes!("fixtures/streaming-agent.sh")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            dir,
            binary,
            provider,
        }
    }

    fn command(&self, mode: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_due"));
        cmd.args([
            "run",
            "--provider",
            self.provider,
            "--prompt",
            "SECRET_PROMPT",
            "--output",
            "json",
            "--binary",
        ])
        .arg(&self.binary)
        .env("DUE_TRACE_DIR", self.dir.path())
        .env("DUE_TRACE_PROVIDER", self.provider)
        .env("DUE_TRACE_MODE", mode)
        .env("DUE_TRACE_SESSION", "trace-session")
        .env("PRIVATE_TEST_TOKEN", "SECRET_ENV")
        .env("RUST_LOG", "trace")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        cmd
    }

    fn trace(&self) -> PathBuf {
        self.dir.path().join("trace.jsonl")
    }

    fn pids(&self) -> Vec<i32> {
        fs::read_to_string(self.dir.path().join("processes"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.parse().ok())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Guard against an assertion leaving a gated fake provider running.
        for pid in self.pids() {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("due exceeded the tracing-test deadline");
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

fn run(cmd: &mut Command) -> Output {
    finish(cmd.spawn().unwrap())
}

fn result(output: &Output, exit: i32, status: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], status);
    value
}

fn records(bytes: &[u8]) -> Vec<Value> {
    let text = std::str::from_utf8(bytes).unwrap();
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("non-JSON trace line {line:?}: {error}"))
        })
        .collect()
}

fn check_trace(events: &[Value], provider: &str, run_id: &Value, status: &str, exit: i32) {
    assert!(!events.is_empty());
    assert!(run_id.as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(events.first().unwrap()["event"], "run_started");
    assert_eq!(events.last().unwrap()["event"], "run_finished");
    assert_eq!(events.last().unwrap()["status"], status);
    assert_eq!(events.last().unwrap()["exit_code"], exit);
    let mut previous = None;
    let mut elapsed = 0;
    for event in events {
        assert_eq!(event["schema_version"], 1);
        assert_eq!(event["provider"], provider);
        assert_eq!(&event["run_id"], run_id);
        let seq = event["seq"].as_u64().unwrap();
        if let Some(previous) = previous {
            assert!(seq > previous, "{events:?}");
        }
        previous = Some(seq);
        let now = event["elapsed_ms"].as_u64().unwrap();
        assert!(now >= elapsed);
        elapsed = now;
    }
}

fn no_secrets(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    assert!(
        !text.contains("SECRET_"),
        "trace leaked private content: {text}"
    );
    assert!(
        !text.contains("FINAL TRACED ANSWER"),
        "trace leaked answer text: {text}"
    );
    assert!(
        !text.contains("PARTIAL ANSWER"),
        "trace leaked partial answer text: {text}"
    );
}

fn wait_ready(fixture: &Fixture, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.dir.path().join("ready").exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "provider exited before ready"
        );
        assert!(Instant::now() < deadline, "provider did not become ready");
        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_stopped(pids: &[i32]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    for &pid in pids {
        while unsafe { libc::kill(pid, 0) } == 0 {
            let output = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&output.stdout)
                .trim_start()
                .starts_with('Z')
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "traced descendant {pid} survived"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn tracing_is_live_heartbeats_while_quiet_and_dual_sinks_are_identical() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let mut child = fixture
            .command("gated")
            .args(["--trace=json", "--trace-interval", "100ms", "--trace-file"])
            .arg(fixture.trace())
            .args(["--", "--custom-option", "SECRET_NATIVE_ARG"])
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                bytes.extend_from_slice(line.as_bytes());
                bytes.push(b'\n');
                let _ = sender.send(line);
            }
            bytes
        });
        wait_ready(&fixture, &mut child);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut tool_started = false;
        let mut heartbeat = false;
        while !(tool_started && heartbeat) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = receiver
                .recv_timeout(remaining)
                .expect("live trace was not flushed before provider completion");
            let event: Value = serde_json::from_str(&line).unwrap();
            tool_started |= event["event"] == "tool_started";
            // A startup heartbeat is insufficient: prove another heartbeat
            // arrives during the deliberately quiet in-flight tool.
            heartbeat |= tool_started && event["event"] == "heartbeat";
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "provider ended before gate release"
        );
        let live_file = fs::read(fixture.trace()).unwrap();
        assert!(String::from_utf8_lossy(&live_file).contains("tool_started"));
        let last_complete_line = live_file.iter().rposition(|byte| *byte == b'\n').unwrap();
        let live_records = records(&live_file[..=last_complete_line]);
        assert!(
            !live_records
                .iter()
                .any(|event| event["event"] == "tool_completed" || event["event"] == "tool_failed"),
            "a streamed tool-input block ending does not mean the tool executed"
        );
        fs::write(fixture.dir.path().join("release"), b"go").unwrap();
        let mut output = finish(child);
        output.stderr = reader.join().unwrap();
        let final_result = result(&output, 0, "succeeded");
        assert_eq!(final_result["text"], "FINAL TRACED ANSWER");
        assert_eq!(final_result["session_id"], "trace-session");
        assert_eq!(final_result["usage"]["input_tokens"], 120);
        let stderr_records = records(&output.stderr);
        let file_bytes = fs::read(fixture.trace()).unwrap();
        let file_records = records(&file_bytes);
        assert_eq!(stderr_records, file_records);
        check_trace(
            &file_records,
            provider,
            &final_result["run_id"],
            "succeeded",
            0,
        );
        for name in [
            "process_started",
            "session_started",
            "turn_started",
            "tool_started",
            "tool_completed",
            "message_completed",
            "turn_completed",
        ] {
            assert!(
                file_records.iter().any(|event| event["event"] == name),
                "missing {name}: {file_records:?}"
            );
        }
        let process = file_records
            .iter()
            .find(|event| event["event"] == "process_started")
            .unwrap();
        assert!(process["pid"].as_u64().is_some_and(|pid| pid > 0));
        let tool = file_records
            .iter()
            .find(|event| event["event"] == "tool_started")
            .unwrap();
        assert_eq!(
            file_records
                .iter()
                .filter(|event| event["event"] == "tool_started")
                .count(),
            1,
            "partial and complete assistant messages must not duplicate tool starts"
        );
        let complete = file_records
            .iter()
            .find(|event| event["event"] == "tool_completed")
            .unwrap();
        assert_eq!(tool["item_id"], complete["item_id"]);
        assert!(!tool["item_id"].is_null());
        no_secrets(&output.stderr);
        no_secrets(&file_bytes);
    }
}

#[test]
fn traced_dry_run_plans_stream_transport_without_starting_a_process() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture
            .command("success")
            .args(["--trace=json", "--dry-run"]));
        let value = result(&output, 0, "planned");
        assert!(!fixture.dir.path().join("argv").exists());
        let events = records(&output.stderr);
        check_trace(&events, provider, &value["run_id"], "planned", 0);
        assert!(
            !events
                .iter()
                .any(|event| event["event"] == "process_started")
        );
        let args = value["invocation"]["args"].as_array().unwrap();
        if provider == "claude" {
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "--output-format" && pair[1] == "stream-json")
            );
        } else {
            assert!(args.iter().any(|arg| arg == "--json"));
        }
        no_secrets(&output.stderr);
        no_secrets(&output.stdout);
    }
}

#[test]
fn trace_file_alone_does_not_print_trace_records_to_stderr() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture
            .command("success")
            .arg("--trace-file")
            .arg(fixture.trace()));
        let value = result(&output, 0, "succeeded");
        let bytes = fs::read(fixture.trace()).unwrap();
        check_trace(&records(&bytes), provider, &value["run_id"], "succeeded", 0);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("run_started") && !stderr.contains("run_finished"),
            "{stderr}"
        );
        no_secrets(&bytes);

        let fixture = Fixture::new(provider);
        let output = run(fixture
            .command("nonzero")
            .arg("--trace-file")
            .arg(fixture.trace()));
        let value = result(&output, 1, "failed");
        let bytes = fs::read(fixture.trace()).unwrap();
        check_trace(&records(&bytes), provider, &value["run_id"], "failed", 1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("SECRET_PROVIDER_STDERR"),
            "file-only tracing must retain ordinary failure diagnostics: {stderr}"
        );
        assert!(!stderr.contains("run_started") && !stderr.contains("run_finished"));
        no_secrets(&bytes);
    }
}

#[test]
fn bare_trace_uses_text_stderr_and_keeps_stdout_json() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture.command("success").arg("--trace"));
        let value = result(&output, 0, "succeeded");
        assert_eq!(value["text"], "FINAL TRACED ANSWER");
        assert!(!output.stderr.is_empty());
        // Text trace need not have fixed punctuation, but is not a JSONL sink.
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .any(|line| serde_json::from_str::<Value>(line).is_err())
        );
    }
}

#[test]
fn tracing_keeps_resume_session_identifiers() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture
            .command("success")
            .args(["--resume", "resumed-session", "--trace=json"])
            .env("DUE_TRACE_SESSION", "resumed-session"));
        let value = result(&output, 0, "succeeded");
        assert_eq!(value["session_id"], "resumed-session");
        let events = records(&output.stderr);
        check_trace(&events, provider, &value["run_id"], "succeeded", 0);
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "session_started"
                    && event["session_id"] == "resumed-session")
        );
        no_secrets(&output.stderr);
    }
}

#[test]
fn a_failed_tool_can_be_followed_by_a_successful_turn() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture.command("tool_failure").arg("--trace=json"));
        let value = result(&output, 0, "succeeded");
        let events = records(&output.stderr);
        check_trace(&events, provider, &value["run_id"], "succeeded", 0);
        let started = events
            .iter()
            .find(|event| event["event"] == "tool_started")
            .unwrap();
        let failed = events
            .iter()
            .find(|event| event["event"] == "tool_failed")
            .expect("failed tool must be distinguishable from successful completion");
        assert_eq!(started["item_id"], failed["item_id"]);
        assert!(!failed["item_id"].is_null());
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "turn_completed")
        );
        no_secrets(&output.stderr);
    }
}

#[test]
fn traced_semantic_and_process_failures_keep_metadata_but_redact_traces() {
    for provider in ["claude", "codex"] {
        for mode in ["semantic", "nonzero"] {
            let fixture = Fixture::new(provider);
            let output = run(fixture.command(mode).arg("--trace=json"));
            let value = result(&output, 1, "failed");
            assert_eq!(value["text"], "PARTIAL ANSWER");
            assert_eq!(value["session_id"], "trace-session");
            assert_eq!(value["error"]["kind"], "provider");
            if mode == "nonzero" {
                assert_eq!(value["error"]["provider_exit_code"], 9);
            }
            if provider == "claude" {
                assert_eq!(value["cost_usd"], 0.0123);
            }
            let events = records(&output.stderr);
            check_trace(&events, provider, &value["run_id"], "failed", 1);
            assert!(events.iter().any(|event| event["event"] == "turn_failed"));
            assert_eq!(events.last().unwrap()["error_kind"], "provider");
            no_secrets(&output.stderr);
        }
    }
}

#[test]
fn existing_or_uncreatable_trace_files_fail_before_provider_spawn() {
    for provider in ["claude", "codex"] {
        for existing in [true, false] {
            let fixture = Fixture::new(provider);
            let path = if existing {
                fixture.trace()
            } else {
                fixture.dir.path().join("missing-parent/trace.jsonl")
            };
            if existing {
                fs::write(&path, b"retain existing trace\n").unwrap();
            }
            let output = run(fixture.command("success").arg("--trace-file").arg(&path));
            result(&output, 1, "failed");
            assert!(!fixture.dir.path().join("argv").exists());
            if existing {
                assert_eq!(fs::read(&path).unwrap(), b"retain existing trace\n");
            }
        }
    }
}

#[test]
fn trace_interval_requires_enabled_tracing_and_positive_duration() {
    let fixture = Fixture::new("claude");
    for args in [
        vec!["--trace-interval", "100ms"],
        vec!["--trace", "--trace-interval", "0s"],
        vec!["--trace", "--trace-interval", "invalid"],
        vec!["--trace", "json"],
    ] {
        let output = run(fixture.command("success").args(&args));
        assert_eq!(
            output.status.code(),
            Some(2),
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!fixture.dir.path().join("argv").exists());
}

#[test]
fn concurrent_trace_files_have_distinct_run_ids() {
    let claude = Fixture::new("claude");
    let codex = Fixture::new("codex");
    let mut first = claude
        .command("gated")
        .arg("--trace-file")
        .arg(claude.trace())
        .spawn()
        .unwrap();
    let mut second = codex
        .command("gated")
        .arg("--trace-file")
        .arg(codex.trace())
        .spawn()
        .unwrap();
    wait_ready(&claude, &mut first);
    wait_ready(&codex, &mut second);
    assert!(first.try_wait().unwrap().is_none() && second.try_wait().unwrap().is_none());
    fs::write(claude.dir.path().join("release"), b"go").unwrap();
    fs::write(codex.dir.path().join("release"), b"go").unwrap();
    let left = result(&finish(first), 0, "succeeded");
    let right = result(&finish(second), 0, "succeeded");
    assert_ne!(left["run_id"], right["run_id"]);
    check_trace(
        &records(&fs::read(claude.trace()).unwrap()),
        "claude",
        &left["run_id"],
        "succeeded",
        0,
    );
    check_trace(
        &records(&fs::read(codex.trace()).unwrap()),
        "codex",
        &right["run_id"],
        "succeeded",
        0,
    );
}

#[test]
fn traced_timeout_and_ctrl_c_report_final_outcome_after_process_cleanup() {
    for provider in ["claude", "codex"] {
        for cancel in [false, true] {
            let fixture = Fixture::new(provider);
            let mut child = fixture
                .command("blocked")
                .args([
                    "--trace=json",
                    "--timeout",
                    if cancel { "30s" } else { "2s" },
                ])
                .spawn()
                .unwrap();
            wait_ready(&fixture, &mut child);
            let pids = fixture.pids();
            assert_eq!(pids.len(), 2);
            if cancel {
                assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
            }
            let output = finish(child);
            let (code, status, kind) = if cancel {
                (130, "cancelled", "cancelled")
            } else {
                (124, "timed_out", "timeout")
            };
            let value = result(&output, code, status);
            let events = records(&output.stderr);
            check_trace(&events, provider, &value["run_id"], status, code);
            assert_eq!(events.last().unwrap()["error_kind"], kind);
            no_secrets(&output.stderr);
            assert_stopped(&pids);
        }
    }
}

#[test]
fn unread_trace_stderr_does_not_block_cancellation_cleanup_or_final_stdout() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let mut child = fixture
            .command("flood")
            .args(["--trace=json", "--timeout", "30s"])
            .spawn()
            .unwrap();
        // Leave stderr deliberately unread. Ten thousand native messages create
        // far more trace output than the pipe and bounded trace queue can hold.
        wait_ready(&fixture, &mut child);
        let pids = fixture.pids();
        assert_eq!(pids.len(), 2);
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        let deadline = Instant::now() + Duration::from_secs(12);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{provider}: blocked trace stderr prevented exit after cancellation");
            }
            thread::sleep(Duration::from_millis(20));
        }
        // Only drain the pipe after the process has independently terminated.
        let output = child.wait_with_output().unwrap();
        let value = result(&output, 130, "cancelled");
        assert!(value["run_id"].is_string());
        assert_eq!(value["trace"]["incomplete"], true);
        assert!(
            value["trace"]["dropped_records"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
        assert_stopped(&pids);
        no_secrets(&output.stderr);
        // A record may be partially written when the drain budget expires;
        // transport shutdown must not wait indefinitely to complete that line.
        assert!(!output.stderr.is_empty());
    }
}
