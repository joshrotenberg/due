#![cfg(unix)]

use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};
use tempfile::tempdir;

fn run(mode: &str, trace: bool) -> (Value, Vec<Value>, i32) {
    let dir = tempdir().unwrap();
    let binary = dir.path().join("fake claude");
    fs::write(&binary, include_bytes!("fixtures/claude-continuation.sh")).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
    command
        .args([
            "run",
            "--provider",
            "claude",
            "--prompt",
            "x",
            "--output",
            "json",
            "--binary",
        ])
        .arg(&binary)
        .env("DUE_CONTINUATION_MODE", mode)
        .stdin(Stdio::null());
    if trace {
        command.arg("--trace=json");
    }
    if mode == "timeout_continuation" {
        command.args(["--timeout", "1s"]);
    }
    let output = command.output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
    let events = if trace {
        output
            .stderr
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect()
    } else {
        Vec::new()
    };
    (value, events, output.status.code().unwrap())
}

fn completion_phases(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event["event"].as_str())
        .filter(|name| *name == "turn_completed" || *name == "turn_failed")
        .collect()
}

#[test]
fn unfinished_tail_and_invalid_final_keep_last_valid_evidence() {
    for trace in [false, true] {
        for mode in ["unfinished", "malformed_final"] {
            let (value, events, exit) = run(mode, trace);
            assert_eq!(exit, 1, "{mode}: {value}");
            assert_eq!(value["error"]["kind"], "protocol", "{mode}: {value}");
            assert_eq!(value["text"], "PHASE_ONE", "{mode}: {value}");
            assert_eq!(value["session_id"], "first");
            assert_eq!(value["cost_usd"], 0.01);
            assert_eq!(value["usage"]["input_tokens"], 4);
            assert_eq!(value["usage"]["output_tokens"], 11);
            if mode == "malformed_final" {
                assert_eq!(value["structured_output"], serde_json::json!({"answer": 1}));
            }
            assert!(completion_phases(&events).is_empty());
        }
    }
}

#[test]
fn invalid_only_result_preserves_native_structured_output() {
    for trace in [false, true] {
        let (value, events, exit) = run("invalid_only_structured", trace);
        assert_eq!(exit, 1, "{value}");
        assert_eq!(value["error"]["kind"], "protocol");
        assert_eq!(
            value["structured_output"],
            serde_json::json!({"kept": "native-json"})
        );
        assert!(value["text"].as_str().is_some_and(str::is_empty));
        assert!(value["usage"].is_null());
        assert!(completion_phases(&events).is_empty());
    }
}

#[test]
fn invalid_final_keeps_structured_evidence_and_bookkeeping_does_not_open_tail() {
    for trace in [false, true] {
        let (value, events, _) = run("bookkeeping", trace);
        assert_eq!(value["status"], "succeeded");
        assert_eq!(value["text"], "PHASE_TWO");
        if trace {
            assert_eq!(completion_phases(&events), ["turn_completed"]);
        }
    }
}

#[test]
fn caps_auth_and_native_exit_agree_across_modes() {
    for trace in [false, true] {
        for (mode, kind) in [
            ("cap_after_malformed", "max_turns"),
            ("budget_after_malformed", "max_budget"),
            ("auth_after_malformed", "authentication"),
        ] {
            let (value, _, exit) = run(mode, trace);
            assert_eq!(exit, 1, "{mode}: {value}");
            assert_eq!(value["error"]["kind"], kind, "{mode}: {value}");
        }
        for mode in ["exit_without_terminal", "exit_with_invalid_terminal"] {
            let (value, events, exit) = run(mode, trace);
            assert_eq!(exit, 1, "{mode}: {value}");
            assert_eq!(value["error"]["kind"], "provider", "{mode}: {value}");
            assert_eq!(value["error"]["provider_exit_code"], 9);
            assert!(completion_phases(&events).is_empty());
        }
        let (value, events, exit) = run("failed_after_malformed", trace);
        assert_eq!(exit, 1, "{value}");
        assert_eq!(value["error"]["kind"], "protocol");
        assert!(completion_phases(&events).is_empty());
    }
}

#[test]
fn interrupted_continuation_has_no_completion_phase() {
    let (value, events, exit) = run("timeout_continuation", true);
    assert_eq!(exit, 124, "{value}");
    assert_eq!(value["error"]["kind"], "timeout");
    assert!(completion_phases(&events).is_empty());
}

#[test]
fn observed_verbose_array_and_stream_jsonl_sum_segments() {
    for trace in [false, true] {
        let (value, events, exit) = run("success", trace);
        assert_eq!(exit, 0);
        assert_eq!(value["text"], "PHASE_TWO");
        assert_eq!(value["session_id"], "last");
        assert_eq!(value["cost_usd"], 0.03);
        assert_eq!(value["usage"]["input_tokens"], 6);
        assert_eq!(value["usage"]["cached_input_tokens"], 10);
        assert_eq!(value["usage"]["cache_write_input_tokens"], 14);
        assert_eq!(value["usage"]["output_tokens"], 24);
        assert!(value["usage"]["total_tokens"].is_null());
        if trace {
            let phases: Vec<_> = events
                .iter()
                .filter_map(|event| event["event"].as_str())
                .filter(|name| {
                    name.starts_with("turn_completed") || name.starts_with("turn_failed")
                })
                .collect();
            assert_eq!(phases, ["turn_completed"]);
            assert_eq!(events.last().unwrap()["event"], "run_finished");
        }
    }
}

#[test]
fn failure_exit_and_missing_counts_keep_native_data() {
    for trace in [false, true] {
        let (value, events, exit) = run("failed", trace);
        assert_eq!(exit, 1);
        assert_eq!(value["error"]["kind"], "provider");
        assert_eq!(value["text"], "PHASE_TWO");
        assert_eq!(
            value["structured_output"],
            serde_json::json!({"partial": true})
        );
        if trace {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event["event"] == "turn_failed")
                    .count(),
                1
            );
        }
        let (value, _, _) = run("missing", trace);
        assert!(value["usage"]["input_tokens"].is_null());
        assert!(value["usage"]["cached_input_tokens"].is_null());
        assert_eq!(value["usage"]["output_tokens"], 24);
        let (value, _, exit) = run("exit9", trace);
        assert_eq!(exit, 1);
        assert_eq!(value["error"]["provider_exit_code"], 9);
        assert_eq!(value["text"], "PHASE_TWO");
    }
}

#[test]
fn child_adjacent_malformed_and_overflow_are_bounded() {
    for trace in [false, true] {
        let (child, _, exit) = run("child", trace);
        assert_eq!(exit, 0);
        assert_eq!(child["usage"]["output_tokens"], 24);
        for mode in ["adjacent", "malformed", "overflow"] {
            let (value, _, exit) = run(mode, trace);
            assert_eq!(exit, 1, "{mode}: {value}");
            assert_eq!(value["error"]["kind"], "protocol", "{mode}: {value}");
            if mode == "overflow" {
                assert_eq!(value["text"], "PHASE_ONE");
                assert_eq!(value["usage"]["output_tokens"], u64::MAX);
            }
        }
    }
}
