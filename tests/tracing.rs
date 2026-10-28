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
        Self::with_script(provider, include_bytes!("fixtures/streaming-agent.sh"))
    }

    fn parsing(provider: &'static str) -> Self {
        Self::with_script(provider, include_bytes!("fixtures/parsing-agent.sh"))
    }

    fn with_script(provider: &'static str, script: &[u8]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("streaming-provider");
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            dir,
            binary,
            provider,
        }
    }

    fn command(&self, mode: &str) -> Command {
        self.command_output(mode, "json")
    }

    fn command_output(&self, mode: &str, output: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_due"));
        cmd.args([
            "run",
            "--provider",
            self.provider,
            "--prompt",
            "SECRET_PROMPT",
            "--output",
            output,
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
        if thread::panicking() {
            self.dir.disable_cleanup(true);
            eprintln!("retained failing fixture: {}", self.dir.path().display());
        }
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
fn codex_buffered_and_file_trace_share_line_admission_and_failure_gates() {
    for resume in [false, true] {
        for mode in [
            "valid",
            "ignored",
            "utf8",
            "utf8_stderr",
            "replacement",
            "no_newline",
            "crlf",
            "bad_first",
            "bad_before",
            "truncated_before",
            "bad_after",
            "truncated_after",
            "bad_before_exit9",
            "bad_after_exit9",
            "eof",
            "eof9",
            "no_message",
            "exit9",
            "failed",
            "missing_file",
        ] {
            let success = matches!(
                mode,
                "valid"
                    | "ignored"
                    | "utf8"
                    | "utf8_stderr"
                    | "replacement"
                    | "no_newline"
                    | "crlf"
            );
            for traced in [false, true] {
                let fixture = Fixture::parsing("codex");
                let mut command = fixture.command(mode);
                if resume {
                    command.args(["--resume", "trace-session"]);
                }
                if traced {
                    command.arg("--trace-file").arg(fixture.trace());
                }
                let output = run(&mut command);
                let value = result(
                    &output,
                    i32::from(!success),
                    if success { "succeeded" } else { "failed" },
                );
                assert_eq!(
                    value["text"],
                    if matches!(mode, "bad_first" | "no_message") {
                        ""
                    } else {
                        "PARTIAL ANSWER"
                    },
                    "{mode} traced={traced} resume={resume}: {value}"
                );
                assert_eq!(
                    value["session_id"],
                    if mode == "bad_first" {
                        Value::Null
                    } else {
                        Value::from("trace-session")
                    }
                );
                let has_usage = matches!(
                    mode,
                    "valid"
                        | "ignored"
                        | "utf8"
                        | "utf8_stderr"
                        | "replacement"
                        | "no_newline"
                        | "crlf"
                        | "bad_after"
                        | "truncated_after"
                        | "bad_after_exit9"
                        | "exit9"
                        | "missing_file"
                );
                assert_eq!(!value["usage"].is_null(), has_usage, "{mode}: {value}");
                if has_usage {
                    assert_eq!(value["usage"]["output_tokens"], 3);
                }
                if !success {
                    let native9 = matches!(
                        mode,
                        "eof9" | "exit9" | "failed" | "bad_before_exit9" | "bad_after_exit9"
                    );
                    // Streaming cleanup after bad JSON can precede natural exit9.
                    let parser_cleanup =
                        traced && matches!(mode, "bad_before_exit9" | "bad_after_exit9");
                    assert_eq!(
                        value["error"]["kind"],
                        if native9 && !parser_cleanup {
                            "provider"
                        } else {
                            "protocol"
                        },
                        "{mode}: {value}"
                    );
                    assert_eq!(
                        value["error"]["provider_exit_code"],
                        if native9 && !parser_cleanup {
                            Value::from(9)
                        } else {
                            Value::Null
                        }
                    );
                }
                if traced {
                    let bytes = fs::read(fixture.trace()).unwrap();
                    let events = records(&bytes);
                    check_trace(
                        &events,
                        "codex",
                        &value["run_id"],
                        if success { "succeeded" } else { "failed" },
                        i32::from(!success),
                    );
                    assert_eq!(value["trace"]["incomplete"], false);
                    assert_eq!(value["trace"]["dropped_records"], 0);
                    no_secrets(&bytes);
                    assert!(!String::from_utf8_lossy(&bytes).contains("SECRET_IGNORED"));
                } else {
                    assert!(
                        String::from_utf8_lossy(&output.stderr).contains("SECRET_PROVIDER_STDERR")
                    );
                    if mode == "utf8_stderr" {
                        assert!(
                            String::from_utf8_lossy(&output.stderr)
                                .contains("invalid stderr: \u{fffd}")
                        );
                    }
                }
                assert_stopped(&fixture.pids());
            }
        }
    }
}

#[test]
fn claude_complete_message_fallback_is_latest_text_only_and_terminal_is_authoritative() {
    for (mode, text, kind, exit) in [
        ("eof", "PARTIAL ANSWER", "protocol", 1),
        ("eof9", "PARTIAL ANSWER", "provider", 1),
        ("string", "PARTIAL ANSWER", "protocol", 1),
        ("bad", "PARTIAL ANSWER", "protocol", 1),
        ("child_after_root", "PARTIAL ANSWER", "protocol", 1),
        ("child_only", "", "protocol", 1),
        ("root_null", "ROOT NULL ANSWER", "protocol", 1),
        ("root_after_child", "ROOT NULL ANSWER", "protocol", 1),
        ("invalid_parent", "PARTIAL ANSWER", "protocol", 1),
        ("empty_parent", "PARTIAL ANSWER", "protocol", 1),
        ("invalid_terminal", "", "protocol", 1),
        ("empty", "", "protocol", 1),
        ("no_text", "", "protocol", 1),
        ("success", "TERMINAL ANSWER", "", 0),
        ("success9", "TERMINAL ANSWER", "provider", 1),
        ("empty_success", "", "", 0),
        ("failed", "TERMINAL FAILURE", "provider", 1),
        ("failed9", "TERMINAL FAILURE", "provider", 1),
        ("empty_failed", "", "provider", 1),
        ("max_turns", "", "max_turns", 1),
        ("max_budget", "", "max_budget", 1),
        ("structured", "{\"answer\":42}", "", 0),
    ] {
        let fixture = Fixture::parsing("claude");
        let output = run(fixture
            .command(mode)
            .arg("--trace-file")
            .arg(fixture.trace()));
        let status = if exit == 0 { "succeeded" } else { "failed" };
        let value = result(&output, exit, status);
        assert_eq!(value["text"], text, "{mode}: {value}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("SECRET_SUBAGENT"));
        assert_eq!(value["session_id"], "trace-session");
        assert_eq!(value["structured_output_present"], mode == "structured");
        if !kind.is_empty() {
            assert_eq!(value["error"]["kind"], kind, "{mode}: {value}");
        }
        assert_eq!(
            value["error"]["provider_exit_code"],
            if matches!(
                mode,
                "eof9" | "success9" | "failed9" | "max_turns" | "max_budget"
            ) {
                Value::from(9)
            } else {
                Value::Null
            }
        );
        let bytes = fs::read(fixture.trace()).unwrap();
        check_trace(&records(&bytes), "claude", &value["run_id"], status, exit);
        assert_eq!(value["trace"]["incomplete"], false);
        no_secrets(&bytes);
        assert!(!String::from_utf8_lossy(&bytes).contains("SECRET_SUBAGENT"));
        assert!(!String::from_utf8_lossy(&bytes).contains("TERMINAL"));
        assert_stopped(&fixture.pids());
    }
}

#[test]
fn partial_schema_intent_and_text_mode_failure_remain_truthful() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::parsing(provider);
        let schema = fixture.dir.path().join("answer.schema.json");
        fs::write(&schema, r#"{"type":"object"}"#).unwrap();
        let mode = if provider == "claude" {
            "json_partial"
        } else {
            "bad_after"
        };
        let output = run(fixture
            .command(mode)
            .arg("--trace-file")
            .arg(fixture.trace())
            .arg("--schema")
            .arg(&schema));
        let value = result(&output, 1, "failed");
        assert_eq!(value["answer_schema_validation"], "not_performed");
        assert_eq!(value["structured_output_present"], false);
        assert_eq!(value["error"]["kind"], "protocol");
        if provider == "claude" {
            assert_eq!(value["text"], "{\"answer\":42}");
        }
        // Use the same complete partial content with the text output mode.
        let mut command = fixture.command_output(mode, "text");
        if provider == "claude" {
            command.arg("--trace");
        }
        let output = run(&mut command);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_stopped(&fixture.pids());
    }
}

#[test]
fn claude_partial_message_survives_timeout_and_cancel_after_receipt() {
    for cancel in [false, true] {
        let fixture = Fixture::parsing("claude");
        let mut child = fixture
            .command(if cancel { "cancel" } else { "timeout" })
            .arg("--trace-file")
            .arg(fixture.trace())
            .args(["--timeout", if cancel { "30s" } else { "2s" }])
            .spawn()
            .unwrap();
        wait_ready(&fixture, &mut child);
        let deadline = Instant::now() + Duration::from_secs(5);
        // Provider writes alone are not a receipt barrier. Wait for Due to
        // acknowledge the complete text-bearing message in its flushed trace.
        loop {
            let bytes = fs::read(fixture.trace()).unwrap_or_default();
            let complete = bytes
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(&b""[..], |end| &bytes[..=end]);
            if records(complete)
                .iter()
                .any(|event| event["event"] == "message_completed" && event["message_bytes"] == 14)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "message receipt barrier timed out"
            );
            assert!(child.try_wait().unwrap().is_none());
            thread::sleep(Duration::from_millis(20));
        }
        if cancel {
            assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        }
        let output = finish(child);
        let (exit, status, kind) = if cancel {
            (130, "cancelled", "cancelled")
        } else {
            (124, "timed_out", "timeout")
        };
        let value = result(&output, exit, status);
        assert_eq!(value["text"], "PARTIAL ANSWER");
        assert_eq!(value["error"]["kind"], kind);
        assert_eq!(value["session_id"], "trace-session");
        assert_eq!(value["structured_output_present"], false);
        let bytes = fs::read(fixture.trace()).unwrap();
        check_trace(&records(&bytes), "claude", &value["run_id"], status, exit);
        no_secrets(&bytes);
        assert_eq!(fixture.pids().len(), 2);
        assert_stopped(&fixture.pids());
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
            tool_started |= event["event"] == "activity_started";
            // A startup heartbeat is insufficient: prove another heartbeat
            // arrives during the deliberately quiet in-flight tool.
            heartbeat |= tool_started && event["event"] == "heartbeat";
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "provider ended before gate release"
        );
        let live_file = fs::read(fixture.trace()).unwrap();
        assert!(String::from_utf8_lossy(&live_file).contains("activity_started"));
        let last_complete_line = live_file.iter().rposition(|byte| *byte == b'\n').unwrap();
        let live_records = records(&live_file[..=last_complete_line]);
        assert!(
            !live_records
                .iter()
                .any(|event| event["event"] == "activity_completed"),
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
            "activity_started",
            "activity_completed",
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
            .find(|event| event["event"] == "activity_started")
            .unwrap();
        assert_eq!(
            file_records
                .iter()
                .filter(|event| event["event"] == "activity_started")
                .count(),
            1,
            "partial and complete assistant messages must not duplicate tool starts"
        );
        let complete = file_records
            .iter()
            .find(|event| event["event"] == "activity_completed")
            .unwrap();
        assert_eq!(tool["activity_id"], complete["activity_id"]);
        assert!(!tool["activity_id"].is_null());
        assert_eq!(tool["activity"], "command");
        assert_eq!(complete["status"], "succeeded");
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
            .find(|event| event["event"] == "activity_started")
            .unwrap();
        let failed = events
            .iter()
            .find(|event| event["event"] == "activity_completed" && event["status"] == "failed")
            .expect("failed tool must be distinguishable from successful completion");
        assert_eq!(started["activity_id"], failed["activity_id"]);
        assert!(!failed["activity_id"].is_null());
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "turn_completed")
        );
        no_secrets(&output.stderr);
    }
}

#[test]
fn provider_activities_share_categories_ids_and_completion_outcomes() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture.command("diverse").arg("--trace=json"));
        let value = result(&output, 0, "succeeded");
        let events = records(&output.stderr);
        check_trace(&events, provider, &value["run_id"], "succeeded", 0);
        let expected: &[&str] = if provider == "claude" {
            &[
                "command",
                "file_change",
                "mcp_call",
                "web_search",
                "plan_update",
                "tool_call",
            ]
        } else {
            &[
                "command",
                "file_change",
                "mcp_call",
                "web_search",
                "plan_update",
                "status_update",
                "unknown",
            ]
        };
        for activity in expected {
            assert!(
                events
                    .iter()
                    .any(|event| event["event"] == "activity_started"
                        && event["activity"] == *activity),
                "{provider} did not classify {activity}: {events:?}"
            );
        }
        for completed in events
            .iter()
            .filter(|event| event["event"] == "activity_completed")
        {
            let id = &completed["activity_id"];
            assert!(id.is_string());
            assert!(
                events
                    .iter()
                    .any(|started| started["event"] == "activity_started"
                        && started["activity_id"] == *id
                        && started["activity"] == completed["activity"])
            );
            assert!(
                ["succeeded", "failed", "cancelled", "unknown"]
                    .contains(&completed["status"].as_str().unwrap())
            );
        }
        if provider == "codex" {
            assert!(
                events
                    .iter()
                    .any(|event| event["event"] == "activity_completed"
                        && event["status"] == "cancelled")
            );
            assert!(events.iter().any(
                |event| event["event"] == "activity_completed" && event["status"] == "unknown"
            ));
        }
        assert!(!events.iter().any(|event| {
            ["tool_started", "tool_completed", "tool_failed"]
                .contains(&event["event"].as_str().unwrap())
        }));
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
fn codex_error_items_are_redacted_diagnostics_not_activities() {
    for (mode, exit, status) in [
        ("diagnostic", 0, "succeeded"),
        ("diagnostic_failed", 1, "failed"),
    ] {
        for detail in [false, true] {
            let fixture = Fixture::new("codex");
            let mut command = fixture.command(mode);
            command
                .args(["--trace=json", "--trace-file"])
                .arg(fixture.trace());
            if detail {
                command.arg("--trace-detail");
            }
            let output = run(&mut command);
            let value = result(&output, exit, status);
            if exit == 0 {
                assert_eq!(value["text"], "FINAL TRACED ANSWER");
            } else {
                assert_eq!(value["error"]["kind"], "provider");
                assert_eq!(value["error"]["message"], "SECRET_PROVIDER_ERROR");
            }
            let file_bytes = fs::read(fixture.trace()).unwrap();
            let events = records(&file_bytes);
            check_trace(&events, "codex", &value["run_id"], status, exit);
            // One phase per completion; started and updated diagnostics are
            // ignored, including when a diagnostic completes without a start.
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event["event"] == "provider_error")
                    .count(),
                2,
                "{events:?}"
            );
            assert_eq!(
                events.iter().any(|event| event["event"] == "turn_failed"),
                exit != 0
            );
            for event in &events {
                if event["event"] == "provider_error" {
                    assert!(event.get("detail").is_none() && event.get("message").is_none());
                }
                assert_ne!(event["activity"], "unknown", "{event:?}");
            }
            let finished = events.last().unwrap();
            assert_eq!(finished["activities_started"], 1);
            assert_eq!(finished["activities_completed"], 1);
            assert_eq!(finished["activities_failed"], 0);

            let summary = run(Command::new(env!("CARGO_BIN_EXE_due"))
                .args(["trace", "summarize", "--output", "json"])
                .arg(fixture.trace())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()));
            assert_eq!(summary.status.code(), Some(0));
            let summary: Value = serde_json::from_slice(&summary.stdout).unwrap();
            let summarized = &summary["runs"][0];
            assert_eq!(summarized["status"], status);
            assert_eq!(summarized["event_counts"]["provider_error"], 2);
            let activities = &summarized["activities"];
            assert_eq!(activities["started"], 1);
            assert_eq!(activities["completed"], 1);
            assert_eq!(activities["failed"], 0);
            assert_eq!(activities["unmatched_starts"], serde_json::json!([]));
            assert_eq!(activities["completions_without_observed_start"], 0);
            assert!(activities["by_kind"].get("unknown").is_none());

            for bytes in [&output.stdout, &output.stderr, &file_bytes] {
                let text = String::from_utf8_lossy(bytes);
                assert!(!text.contains("SECRET_DIAGNOSTIC_MESSAGE"), "{text}");
            }
            if !detail {
                no_secrets(&output.stderr);
                no_secrets(&file_bytes);
            }
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

#[test]
fn detail_is_opt_in_bounded_and_excludes_reasoning_prompts_and_results() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let baseline = run(fixture.command("diverse").arg("--trace=json"));
        let baseline_result = result(&baseline, 0, "succeeded");
        assert!(
            records(&baseline.stderr)
                .iter()
                .all(|r| r.get("detail").is_none())
        );
        no_secrets(&baseline.stderr);

        let output = run(fixture
            .command("diverse")
            .args(["--trace=json", "--trace-detail"]));
        let detailed_result = result(&output, 0, "succeeded");
        assert_eq!(baseline_result["text"], detailed_result["text"]);
        let events = records(&output.stderr);
        let text = String::from_utf8(output.stderr).unwrap();
        assert!(text.contains("SECRET_TOOL_INPUT"));
        assert!(text.contains("FINAL TRACED ANSWER"));
        if provider == "codex" {
            assert!(text.contains("SECRET_FILE_PATH"));
        }
        for forbidden in [
            "SECRET_PROMPT",
            "SECRET_ENV",
            "SECRET_PROVIDER_STDERR",
            "SECRET_TOOL_OUTPUT",
            "SECRET_ACTIVITY_OUTPUT",
            "SECRET_DELTA",
            "SECRET_REASONING_TEXT",
            "SECRET_UNKNOWN",
            "SECRET_PLAN_TEXT",
        ] {
            assert!(!text.contains(forbidden), "{provider}: {forbidden}");
        }
        for event in events {
            if let Some(detail) = event.get("detail") {
                assert!(detail["text"].as_str().unwrap().len() <= 240);
                assert!(detail["truncated"].is_boolean());
            }
        }
    }
}

#[test]
fn detail_is_utf8_safe_and_terminal_controls_are_visible_escapes() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let label = format!("printf \u{1b}[31m\n\t\u{202e}{}", "é🙂".repeat(100));
        // Emit JSON as a printf argument. Replacing the label in the fixture's
        // existing printf format string would expand JSON \n/\t escapes and
        // create malformed native records before Due could inspect them.
        let event = if provider == "claude" {
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[
                {"type":"tool_use","id":"detail-controls","name":"Bash","input":{"command":label}}
            ]}})
        } else {
            serde_json::json!({"type":"item.started","item":{"id":"detail-controls","type":"command_execution","command":label,"status":"in_progress"}})
        };
        let injection = format!("printf '%s\\n' '{event}'\ncase \"$DUE_TRACE_MODE\" in");
        let script = fs::read_to_string(&fixture.binary).unwrap().replacen(
            "case \"$DUE_TRACE_MODE\" in",
            &injection,
            1,
        );
        fs::write(&fixture.binary, script).unwrap();
        let output = run(fixture
            .command("success")
            .args(["--trace=json", "--trace-detail"]));
        result(&output, 0, "succeeded");
        let events = records(&output.stderr);
        let details: Vec<_> = events.iter().filter_map(|e| e.get("detail")).collect();
        let command = details
            .iter()
            .find(|d| d["text"].as_str().unwrap().starts_with("printf"))
            .unwrap();
        let text = command["text"].as_str().unwrap();
        assert!(text.len() <= 240);
        assert_eq!(command["truncated"], true);
        assert!(text.contains("\\u{1b}"));
        assert!(text.contains("\\u{202e}"));
        assert!(!text.chars().any(char::is_control));
        let human = run(fixture
            .command("success")
            .args(["--trace", "--trace-detail"]));
        result(&human, 0, "succeeded");
        assert!(!human.stderr.contains(&0x1b));
        assert!(
            String::from_utf8(human.stderr)
                .unwrap()
                .contains("truncated")
        );
    }
}

#[test]
fn empty_late_claude_detail_does_not_emit_a_record_without_detail() {
    let fixture = Fixture::new("claude");
    let script = fs::read_to_string(&fixture.binary)
        .unwrap()
        .replace("SECRET_TOOL_INPUT", "");
    fs::write(&fixture.binary, script).unwrap();
    let output = run(fixture
        .command("success")
        .args(["--trace=json", "--trace-detail"]));
    result(&output, 0, "succeeded");
    for event in records(&output.stderr) {
        if event["event"] == "activity_detail" {
            assert!(event.get("detail").is_some());
        }
    }
}

#[test]
fn codex_detail_ignores_reasoning_and_unknown_content_blocks() {
    let fixture = Fixture::new("codex");
    let event = serde_json::json!({"type":"item.completed","item":{"id":"message-content","type":"agent_message","content":[
        {"type":"reasoning","text":"SECRET_REASONING_BLOCK"},
        {"type":"future_block","text":"SECRET_UNKNOWN_BLOCK"},
        {"type":"text","text":"VISIBLE_PROGRESS"}
    ]}});
    let injection = format!("printf '%s\\n' '{event}'\ncase \"$DUE_TRACE_MODE\" in");
    let script = fs::read_to_string(&fixture.binary).unwrap().replacen(
        "case \"$DUE_TRACE_MODE\" in",
        &injection,
        1,
    );
    fs::write(&fixture.binary, script).unwrap();
    let output = run(fixture
        .command("success")
        .args(["--trace=json", "--trace-detail"]));
    result(&output, 0, "succeeded");
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("VISIBLE_PROGRESS"));
    assert!(!text.contains("SECRET_REASONING_BLOCK"));
    assert!(!text.contains("SECRET_UNKNOWN_BLOCK"));
}

#[test]
fn oversized_trace_interval_fails_before_trace_or_provider_initialization() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture
            .command("success")
            .args([
                "--dry-run",
                "--trace=json",
                "--trace-interval",
                "18446744073709551615s",
                "--trace-file",
            ])
            .arg(fixture.trace()));
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty(), "CLI syntax errors stay on stderr");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("scheduling range"));
        assert!(!stderr.contains("panicked"));
        assert!(!fixture.trace().exists());
        assert!(!fixture.dir.path().join("argv").exists());
    }
}
