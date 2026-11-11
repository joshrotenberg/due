use serde_json::{Value, json};
use std::{
    fs,
    process::{Command, Output},
};
use tempfile::TempDir;

fn record(run: &str, seq: u64, event: &str) -> Value {
    let mut r = json!({"schema_version":1,"run_id":run,"provider":"codex","seq":seq,
        "elapsed_ms":seq*10,"timestamp_ms":1000+seq,"idle_ms":0,"event":event,
        "activities_started":0,"activities_completed":0,"activities_failed":0,"messages":0,"dropped_records":0});
    match event {
        "run_started" => {
            r["resumed"] = json!(false);
            r["ephemeral"] = json!(false);
            r["dry_run"] = json!(false);
        }
        "run_finished" => {
            r["status"] = json!("succeeded");
            r["exit_code"] = json!(0);
            r["duration_ms"] = json!(seq * 10);
        }
        "activity_started" | "activity_completed" => {
            r["activity"] = json!("command");
            r["activity_id"] = json!("tool-1");
            if event == "activity_completed" {
                r["status"] = json!("succeeded");
            }
        }
        "message_completed" => r["message_bytes"] = json!(10),
        _ => (),
    }
    r
}

fn summarize(bytes: &[u8], format: &str) -> Output {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");
    fs::write(&path, bytes).unwrap();
    Command::new(env!("CARGO_BIN_EXE_due"))
        .args(["trace", "summarize"])
        .arg(path)
        .args(["--output", format])
        .env("PATH", "")
        .output()
        .unwrap()
}
fn lines(records: &[Value]) -> Vec<u8> {
    records
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<String>()
        .into_bytes()
}
fn parsed(output: &Output, code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn summarizes_interleaved_runs_and_unknown_events_without_echoing_detail() {
    let mut detail = record("a", 3, "message_completed");
    detail["detail"] = json!({"text":"PRIVATE_DETAIL","truncated":false});
    let events = vec![
        record("a", 1, "run_started"),
        record("b", 1, "activity_completed"),
        record("a", 2, "activity_started"),
        detail,
        record("a", 4, "future_progress"),
        record("a", 5, "activity_completed"),
        record("a", 6, "run_finished"),
    ];
    let output = summarize(&lines(&events), "json");
    let v = parsed(&output, 0);
    assert_eq!(v["records"], 7);
    assert_eq!(v["runs"].as_array().unwrap().len(), 2);
    assert_eq!(v["runs"][0]["status"], "succeeded");
    assert_eq!(v["runs"][0]["event_counts"]["future_progress"], 1);
    assert_eq!(v["runs"][0]["activities"]["unmatched_starts"], json!([]));
    assert_eq!(v["runs"][1]["status"], Value::Null);
    assert_eq!(v["runs"][1]["terminal_records"], 0);
    assert_eq!(
        v["runs"][1]["activities"]["completions_without_observed_start"],
        1
    );
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("PRIVATE_DETAIL")
    );
    let human = summarize(&lines(&[record("b", 1, "heartbeat")]), "text");
    assert!(human.status.success());
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("terminal record absent")
    );
}

#[test]
fn reports_observed_gaps_drops_and_unmatched_activities_without_inventing_failure() {
    let mut dropped = record("a", 5, "heartbeat");
    dropped["dropped_records"] = json!(3);
    let events = vec![
        record("a", 2, "activity_started"),
        record("a", 3, "activity_started"),
        dropped,
        record("a", 4, "future_event"),
    ];
    let v = parsed(&summarize(&lines(&events), "json"), 0);
    let r = &v["runs"][0];
    assert_eq!(r["status"], Value::Null);
    assert_eq!(r["delivery"]["sequence_gap_records"], 2);
    assert_eq!(r["delivery"]["non_increasing_sequences"], 1);
    assert_eq!(r["delivery"]["reported_dropped_records"], 3);
    assert_eq!(r["activities"]["duplicate_starts"], 1);
    assert_eq!(r["activities"]["unmatched_starts"], json!(["tool-1"]));
}

#[test]
fn invalid_records_oversized_lines_and_missing_files_fail_actionably() {
    let mut missing = record("a", 1, "heartbeat");
    missing.as_object_mut().unwrap().remove("messages");
    let mut zero = record("a", 1, "heartbeat");
    zero["seq"] = json!(0);
    let mut bad_terminal = record("a", 1, "run_finished");
    bad_terminal["status"] = json!("maybe");
    for bytes in [
        b"not JSON\n".to_vec(),
        lines(&[missing]),
        lines(&[zero]),
        lines(&[bad_terminal]),
        vec![b'a'; 1024 * 1024 + 1],
    ] {
        let v = parsed(&summarize(&bytes, "json"), 2);
        assert_eq!(v["error"]["kind"], "invalid_input");
        assert!(v["error"]["message"].as_str().unwrap().contains("line 1"));
    }
    let dir = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_due"))
        .args(["trace", "summarize"])
        .arg(dir.path().join("absent"))
        .args(["--output", "json"])
        .output()
        .unwrap();
    assert_eq!(parsed(&output, 1)["error"]["kind"], "io");
    assert_eq!(parsed(&summarize(b"", "json"), 0)["runs"], json!([]));
}

#[test]
fn completion_kinds_and_terminal_conflicts_remain_explicit() {
    let mut unknown = record("a", 1, "activity_completed");
    unknown["activity"] = json!("unknown");
    unknown["status"] = json!("unknown");
    let mut cancelled = record("a", 2, "activity_completed");
    cancelled["status"] = json!("cancelled");
    let newer = record("a", 4, "run_finished");
    let mut older = record("a", 3, "run_finished");
    older["status"] = json!("failed");
    older["exit_code"] = json!(1);
    // Deliberately damaged synthetic log, not native duplicate-terminal evidence.
    let bytes = lines(&[unknown, cancelled, newer, older]);
    let v = parsed(&summarize(&bytes, "json"), 0);
    let run = &v["runs"][0];
    assert_eq!(run["terminal_conflict"], true);
    assert_eq!(run["terminal_records"], 2);
    assert_eq!(run["status"], Value::Null);
    assert_eq!(run["exit_code"], Value::Null);
    assert_eq!(
        run["activities"]["completion_status_counts"],
        json!({"unknown":1,"cancelled":1})
    );
    assert_eq!(
        run["activities"]["by_kind"]["unknown"]["completion_status_counts"]["unknown"],
        1
    );
    let human = summarize(&bytes, "text");
    assert!(human.status.success());
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("unknown: conflicting terminal records (2 terminal records"));
    assert!(!text.contains("terminal record absent"));
}

#[test]
fn text_summaries_keep_success_and_failure_statuses() {
    for (status, code) in [("succeeded", 0), ("failed", 1)] {
        let mut terminal = record("a", 1, "run_finished");
        terminal["status"] = json!(status);
        terminal["exit_code"] = json!(code);
        let human = summarize(&lines(&[terminal]), "text");
        assert!(human.status.success());
        let text = String::from_utf8(human.stdout).unwrap();
        assert!(text.contains(&format!("codex a: {status} (1 terminal records")));
        assert!(!text.contains("unknown"));
    }
}
