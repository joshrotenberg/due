#![cfg(unix)]

use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

fn run(provider: &str, mode: &str, traced: bool) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("fake-provider");
    let pid_file = dir.path().join("pid");
    fs::write(&binary, include_bytes!("fixtures/output-bounds-agent.sh")).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
    command
        .args([
            "run",
            "--provider",
            provider,
            "--prompt",
            "synthetic",
            "--output",
            "json",
            "--binary",
        ])
        .arg(&binary)
        .env("DUE_BOUNDS_MODE", mode)
        .env("DUE_BOUNDS_PROVIDER", provider)
        .env("DUE_BOUNDS_PID", &pid_file);
    if traced {
        command.arg("--trace=json");
    }
    let output = command.output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "provider process survived: {provider} {mode} {traced}"
    );
    assert_eq!(
        output.status.code(),
        Some(if value["error"].is_null() { 0 } else { 1 }),
        "provider={provider} mode={mode} traced={traced}"
    );
    value
}

#[test]
fn cumulative_stream_caps_fail_without_success() {
    // Sequential runs keep peak fixture memory low. Each JSONL frame is far
    // below 1 MiB; the stream exceeds the 64 MiB cumulative ceiling.
    for provider in ["claude", "codex"] {
        for mode in ["stdout_limit", "stderr_limit"] {
            for traced in [false, true] {
                let value = run(provider, mode, traced);
                assert_eq!(value["status"], "failed", "{provider} {mode} {traced}");
                assert_eq!(
                    value["error"]["kind"], "protocol",
                    "{provider} {mode} {traced}"
                );
                assert!(
                    value["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("67108864-byte captured stream limit")
                );
                assert!(value["structured_output"].is_null());
            }
        }
    }
}

#[test]
fn final_file_targets_and_failed_partial() {
    for traced in [false, true] {
        let exact = run("codex", "final_exact", traced);
        assert_eq!(exact["status"], "succeeded");
        assert_eq!(exact["text"].as_str().unwrap().len(), 4 * 1024 * 1024);
        for mode in ["final_over", "final_symlink", "final_fifo"] {
            let value = run("codex", mode, traced);
            assert_eq!(value["error"]["kind"], "protocol", "{mode} {traced}");
            assert_eq!(value["text"], "partial", "{mode} {traced}");
        }
        let failed = run("codex", "failed_over", traced);
        assert_eq!(failed["error"]["kind"], "provider");
        assert_eq!(failed["text"], "partial");
    }
}

#[test]
fn diagnostic_text_is_utf8_bounded() {
    for provider in ["claude", "codex"] {
        for traced in [false, true] {
            let value = run(provider, "diagnostic", traced);
            let message = value["error"]["message"].as_str().unwrap();
            assert!(message.len() <= 16 * 1024, "{provider} {traced}");
            assert!(message.ends_with(" [truncated]"), "{provider} {traced}");
        }
    }
}
