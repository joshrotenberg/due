#![cfg(unix)]

use serde_json::{Value, json};
use std::{fs, os::unix::fs::symlink, path::Path, process::Command};

fn fixture(directory: &std::path::Path) -> std::path::PathBuf {
    let binary = directory.join("provenance-provider");
    // An immutable executable avoids cross-thread fork/exec races with a fresh
    // copied file's writable descriptor, including ETXTBSY on Linux.
    symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provenance-agent.py"),
        &binary,
    )
    .unwrap();
    binary
}

fn result(command: &mut Command, code: i32) -> Value {
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn provenance_run_merges_requests_and_preserves_reports_and_failures() {
    for provider in ["claude", "codex"] {
        let directory = tempfile::tempdir().unwrap();
        let binary = fixture(directory.path());
        let profile = directory.path().join("profile.json");
        fs::write(
            &profile,
            json!({"schema_version":1,"provider":provider,
            "model":"profile-alias","effort":"low"})
            .to_string(),
        )
        .unwrap();
        for traced in [false, true] {
            for prompt in ["reported", "unknown", "fail"] {
                let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
                command
                    .args(["run", "--request"])
                    .arg(&profile)
                    .args([
                        "--prompt",
                        prompt,
                        "--model",
                        "requested-alias",
                        "--effort",
                        "high",
                        "--output",
                        "json",
                        "--binary",
                    ])
                    .arg(&binary);
                if traced {
                    command.arg("--trace=json");
                }
                let value = result(&mut command, if prompt == "fail" { 1 } else { 0 });
                assert_eq!(
                    value["provenance"]["requested"],
                    json!({"model":"requested-alias","effort":"high"})
                );
                let expected = if provider == "claude" && prompt != "unknown" {
                    json!(["native-a", "native-b"])
                } else {
                    json!([])
                };
                assert_eq!(
                    value["provenance"]["reported"],
                    json!({"models":expected,"effort":null,"truncated":false})
                );
                assert_eq!(value["session_id"], "native-id");
                if prompt == "fail" {
                    assert_eq!(value["status"], "failed");
                    assert_eq!(value["error"]["provider_exit_code"], 9);
                    assert_eq!(value["text"], "partial");
                } else {
                    assert_eq!(value["status"], "succeeded");
                    assert_eq!(value["text"], "");
                }
            }
        }
    }
}

#[test]
fn provenance_planned_defaults_pre_spawn_failures_and_rejected_intake() {
    let directory = tempfile::tempdir().unwrap();
    for provider in ["claude", "codex"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .args([
                "run",
                "--provider",
                provider,
                "--prompt",
                "unknown",
                "--output",
                "json",
                "--binary",
            ])
            .arg(directory.path().join("absent-provider"));
        let value = result(&mut command, 127);
        assert_eq!(
            value["provenance"],
            json!({
                "requested":{"model":null,"effort":null},
                "reported":{"models":[],"effort":null,"truncated":false}
            })
        );
        command.args(["--model", "alias", "--effort", "medium", "--dry-run"]);
        let value = result(&mut command, 0);
        assert_eq!(value["status"], "planned");
        assert_eq!(
            value["provenance"]["requested"],
            json!({"model":"alias","effort":"medium"})
        );
        assert_eq!(value["provenance"]["reported"]["models"], json!([]));
        command
            .arg("--cwd")
            .arg(directory.path().join("missing-directory"));
        let value = result(&mut command, 2);
        assert_eq!(value["status"], "failed");
        assert_eq!(value["provenance"]["requested"]["model"], "alias");
    }
    let value = result(
        Command::new(env!("CARGO_BIN_EXE_due"))
            .args(["run", "--request"])
            .arg(directory.path().join("missing-request"))
            .args(["--model", "alias", "--effort", "high", "--output", "json"]),
        2,
    );
    assert!(value["provider"].is_null());
    assert!(value["provenance"]["requested"].is_null());
}

#[cfg(feature = "harness")]
#[test]
fn provenance_survives_resident_interrupt_and_resets_next_turn() {
    let directory = tempfile::tempdir().unwrap();
    let binary = fixture(directory.path());
    let script = directory.path().join("interrupt.js");
    fs::write(
        &script,
        r#"Due.run(async scope => {
            let observe;
            const observed = new Promise(resolve => { observe = resolve; });
            const worker = await scope.open(Due.args, {
                onItem(item) {
                    const event = item.data?.event;
                    if (item.kind === 'native' && event?.type === 'assistant' &&
                        event.message?.model === 'held-native') observe(item.turn_id);
                }
            });
            const held = await worker.start('hold');
            if (await observed !== held) throw Error('native message belongs to another turn');
            const ack = await worker.interrupt(held);
            const cancelled = await worker.wait(held);
            const next = await worker.turn('unknown');
            return {held, ack, cancelled, next};
        });"#,
    )
    .unwrap();
    let value = result(
        Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("script")
            .arg(&script)
            .arg("--evidence")
            .arg(directory.path().join("evidence"))
            .args([
                "--timeout",
                "15s",
                "--args",
                &json!({"provider":"claude","binary":binary,
                    "model":"alias","effort":"high","timeout":"5s"})
                .to_string(),
            ]),
        0,
    );
    let turns = &value["result"];
    assert_eq!(turns["ack"]["interrupt_requested"], true);
    let cancelled = &turns["cancelled"];
    assert_eq!(cancelled["run_id"], turns["held"]);
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["error"]["kind"], "cancelled");
    assert_eq!(cancelled["text"], "partial");
    assert_eq!(
        cancelled["provenance"]["reported"],
        json!({
            "models":["held-native"], "effort":null, "truncated":false
        })
    );
    let next = &turns["next"];
    assert_eq!(next["status"], "succeeded");
    assert!(next["error"].is_null());
    assert_eq!(next["text"], "");
    assert_eq!(
        next["provenance"]["reported"],
        json!({
            "models":[], "effort":null, "truncated":false
        })
    );
    for turn in [cancelled, next] {
        assert_eq!(turn["session_id"], "native-id");
        assert_eq!(
            turn["provenance"]["requested"],
            json!({"model":"alias","effort":"high"})
        );
    }
}

#[cfg(feature = "harness")]
#[test]
fn provenance_javascript_returns_rpc_turn_results_and_resets_reports() {
    for provider in ["claude", "codex"] {
        let directory = tempfile::tempdir().unwrap();
        let binary = fixture(directory.path());
        let profile = directory.path().join("profile.json");
        fs::write(
            &profile,
            json!({"schema_version":1,"provider":provider,
            "model":"profile-alias","effort":"low"})
            .to_string(),
        )
        .unwrap();
        let script = directory.path().join("workflow.js");
        fs::write(
            &script,
            r#"Due.run(async scope => {
            const items = [];
            const worker = await scope.open(Due.args, {onItem(item) { items.push(item); }});
            const first = await worker.turn('reported');
            const second = await worker.turn('unknown');
            const failed = await worker.turn('fail');
            return {first, second, failed, items};
        });"#,
        )
        .unwrap();
        let value = result(
            Command::new(env!("CARGO_BIN_EXE_due"))
                .arg("script")
                .arg(&script)
                .arg("--evidence")
                .arg(directory.path().join("evidence"))
                .args([
                    "--timeout",
                    "15s",
                    "--args",
                    &json!({"request":profile,"binary":binary,
                "effort":"high","timeout":"5s"})
                    .to_string(),
                ]),
            0,
        );
        let turns = &value["result"];
        for name in ["first", "second", "failed"] {
            let turn = &turns[name];
            assert_eq!(
                turn["provenance"]["requested"],
                json!({"model":"profile-alias","effort":"high"})
            );
            let expected = if provider == "claude" && name != "second" {
                json!(["native-a", "native-b"])
            } else {
                json!([])
            };
            assert_eq!(
                turn["provenance"]["reported"],
                json!({"models":expected,"effort":null,"truncated":false})
            );
            assert_eq!(turn["session_id"], "native-id");
            assert_eq!(
                turn["status"],
                if name == "failed" {
                    "failed"
                } else {
                    "succeeded"
                }
            );
            assert!(
                turns["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["kind"] == "turn_finished" && item["data"] == *turn)
            );
        }
        assert_eq!(turns["failed"]["text"], "partial");
        if provider == "claude" {
            assert!(
                turns["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["data"]["event"]["message"]["future"] == 42)
            );
        } else {
            assert!(
                turns["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["data"]["response"]["model"] == "configured")
            );
        }
    }
}
