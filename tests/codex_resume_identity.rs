#![cfg(unix)]

use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn resume_requires_consistent_native_thread_identity_in_buffered_and_traced_runs() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("fake-codex");
    fs::write(&binary, include_bytes!("fixtures/codex-resume-identity.sh")).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

    for traced in [false, true] {
        for (mode, expected_id, expected_error) in [
            ("matching", Some("Requested-Thread"), None),
            ("case_variant", Some("REQUESTED-THREAD"), None),
            ("mismatch", Some("other-thread"), Some("protocol")),
            ("empty", None, Some("protocol")),
            ("missing", None, Some("protocol")),
            ("fallback", Some("unrelated-session"), Some("protocol")),
            ("conflict", Some("Requested-Thread"), Some("protocol")),
            (
                "repeated_conflict",
                Some("Requested-Thread"),
                Some("protocol"),
            ),
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
            command
                .args([
                    "run",
                    "--provider",
                    "codex",
                    "--prompt",
                    "synthetic",
                    "--output",
                    "json",
                    "--resume",
                    "requested-thread",
                ])
                .args(["--binary", binary.to_str().unwrap()])
                .env("DUE_ID_MODE", mode);
            if traced {
                command.arg("--trace=json");
            }
            let output = command.output().unwrap();
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                output.status.code(),
                Some(if expected_error.is_some() { 1 } else { 0 }),
                "mode={mode}, traced={traced}, stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                value["session_id"].as_str(),
                expected_id,
                "mode={mode}, traced={traced}"
            );
            assert_eq!(
                value["error"]["kind"].as_str(),
                expected_error,
                "mode={mode}, traced={traced}"
            );
            assert_eq!(value["text"], "FINAL ANSWER");
            assert_eq!(value["usage"]["input_tokens"], 12);
            assert_eq!(value["usage"]["output_tokens"], 3);
            if traced {
                assert!(value["run_id"].is_string());
            }
        }
        for (mode, expected) in [
            ("auth", "authentication"),
            ("exit", "provider"),
            ("mismatch_then_exit", "provider"),
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
            command
                .args([
                    "run",
                    "--provider",
                    "codex",
                    "--prompt",
                    "synthetic",
                    "--output",
                    "json",
                    "--resume",
                    "requested-thread",
                ])
                .args(["--binary", binary.to_str().unwrap()])
                .env("DUE_ID_MODE", mode);
            if traced {
                command.arg("--trace=json");
            }
            let output = command.output().unwrap();
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value["error"]["kind"], expected,
                "mode={mode}, traced={traced}"
            );
            assert_eq!(
                value["session_id"],
                if mode == "mismatch_then_exit" {
                    Value::String("other-thread".into())
                } else {
                    Value::Null
                }
            );
            assert_eq!(
                value["error"]["provider_exit_code"],
                if mode == "auth" { 3 } else { 9 }
            );
        }
        let schema = dir.path().join("schema.json");
        fs::write(&schema, "false").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .args([
                "run",
                "--provider",
                "codex",
                "--prompt",
                "synthetic",
                "--output",
                "json",
                "--resume",
                "requested-thread",
                "--binary",
            ])
            .arg(&binary)
            .arg("--schema")
            .arg(&schema)
            .env("DUE_ID_MODE", "structured_mismatch");
        if traced {
            command.arg("--trace=json");
        }
        let output = command.output().unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(value["error"]["kind"], "protocol");
        assert_eq!(value["session_id"], "other-thread");
        assert_eq!(value["text"], r#"{"answer":42}"#);
        assert_eq!(value["structured_output"], serde_json::json!({"answer":42}));
        assert_eq!(value["structured_output_present"], true);
        assert_eq!(value["usage"]["input_tokens"], 12);

        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .args([
                "run",
                "--provider",
                "codex",
                "--prompt",
                "synthetic",
                "--output",
                "json",
                "--resume",
                "requested-thread",
                "--binary",
            ])
            .arg(&binary)
            .env("DUE_ID_MODE", "conflict_failed");
        if traced {
            command.arg("--trace=json");
        }
        let output = command.output().unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(value["error"]["kind"], "protocol");
        assert_eq!(value["error"]["provider_exit_code"], 9);
        assert_eq!(value["session_id"], "Requested-Thread");
        assert_eq!(value["text"], "FINAL ANSWER");

        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .args([
                "run",
                "--provider",
                "codex",
                "--prompt",
                "synthetic",
                "--output",
                "json",
                "--resume",
                "requested-thread",
                "--binary",
            ])
            .arg(&binary)
            .env("DUE_ID_MODE", "conflict_completed_exit");
        if traced {
            command.arg("--trace=json");
        }
        let output = command.output().unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(value["error"]["kind"], "provider", "traced={traced}");
        assert_eq!(value["error"]["provider_exit_code"], 9);
        assert_eq!(value["session_id"], "Requested-Thread");
        assert_eq!(value["text"], "FINAL ANSWER");
        assert_eq!(value["usage"]["input_tokens"], 12);
    }
}
