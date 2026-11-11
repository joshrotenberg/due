#![cfg(unix)]

//! Wrapper rate-limit and quota classifications are provider failures, while
//! credential failures remain authentication, with and without tracing.

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    binary: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("fake claude");
        fs::write(&binary, include_bytes!("fixtures/claude-rate-limit.sh")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir, binary }
    }

    fn command(&self, mode: &str, trace: bool) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_due"));
        cmd.args([
            "run",
            "--provider",
            "claude",
            "--prompt",
            "x",
            "--output",
            "json",
            "--binary",
        ])
        .arg(&self.binary)
        .env("DUE_LIMIT_DIR", self.dir.path())
        .env("DUE_LIMIT_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        if trace {
            cmd.arg("--trace=json");
        }
        cmd
    }

    fn pids(&self) -> Vec<i32> {
        fs::read_to_string(self.dir.path().join("pids"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.parse().ok())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
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
            panic!("CLI exceeded test deadline");
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
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

/// With tracing on, stderr holds only trace records and the last one reports
/// the same error kind as the result document.
fn check_trace(output: &Output, trace: bool, kind: &str) {
    if !trace {
        return;
    }
    let events: Vec<Value> = std::str::from_utf8(&output.stderr)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let last = events.last().unwrap();
    assert_eq!(last["event"], "run_finished");
    assert_eq!(last["error_kind"], kind);
}

#[test]
fn rate_limit_and_quota_stderr_are_provider_failures_with_native_diagnostic() {
    for (mode, diagnostic) in [
        ("rate_limit", "API Error: 429"),
        ("quota", "Usage quota exceeded for this organization"),
    ] {
        for trace in [false, true] {
            let fixture = Fixture::new();
            let output = finish(fixture.command(mode, trace).spawn().unwrap());
            let value = result(&output, 1, "failed");
            assert_eq!(value["error"]["kind"], "provider", "{mode} trace={trace}");
            assert_eq!(value["error"]["provider_exit_code"], 2, "{mode}");
            assert!(
                value["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(diagnostic),
                "{mode} trace={trace}: {value}"
            );
            if trace {
                assert_eq!(value["session_id"], "limit-session");
            }
            check_trace(&output, trace, "provider");
        }
    }
}

#[test]
fn credential_failure_stderr_remains_authentication() {
    for (mode, diagnostic) in [
        ("not_logged_in", "Not logged in"),
        ("invalid_key", "Invalid API key"),
    ] {
        for trace in [false, true] {
            let fixture = Fixture::new();
            let output = finish(fixture.command(mode, trace).spawn().unwrap());
            let value = result(&output, 1, "failed");
            assert_eq!(
                value["error"]["kind"], "authentication",
                "{mode} trace={trace}"
            );
            assert_eq!(value["error"]["provider_exit_code"], 3, "{mode}");
            assert!(
                value["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(diagnostic),
                "{mode} trace={trace}: {value}"
            );
            check_trace(&output, trace, "authentication");
        }
    }
}

#[test]
fn explicit_cap_result_precedes_rate_limit_stderr() {
    for trace in [false, true] {
        let fixture = Fixture::new();
        let output = finish(
            fixture
                .command("rate_limit_max_turns", trace)
                .args(["--", "--max-turns", "1"])
                .spawn()
                .unwrap(),
        );
        let value = result(&output, 1, "failed");
        assert_eq!(value["error"]["kind"], "max_turns", "trace={trace}");
        assert_eq!(value["error"]["provider_exit_code"], 1);
        assert_eq!(value["session_id"], "limit-session");
        assert_eq!(value["cost_usd"], 0.08);
        check_trace(&output, trace, "max_turns");
    }
}

#[test]
fn timeout_and_cancellation_precede_rate_limit_stderr() {
    for trace in [false, true] {
        for cancel in [false, true] {
            let fixture = Fixture::new();
            let mut child = fixture
                .command("rate_limit_blocked", trace)
                .args(["--timeout", if cancel { "30s" } else { "2s" }])
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while fixture.pids().len() < 2 {
                assert!(child.try_wait().unwrap().is_none(), "provider exited early");
                assert!(Instant::now() < deadline, "provider did not become ready");
                thread::sleep(Duration::from_millis(20));
            }
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
            assert_eq!(value["error"]["kind"], kind, "trace={trace}");
            check_trace(&output, trace, kind);
        }
    }
}
