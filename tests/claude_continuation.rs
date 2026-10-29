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
        }
    }
}
