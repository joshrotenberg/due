//! Promise-specific ownership rules complement the shared language scenarios.
#![cfg(all(unix, feature = "js"))]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

fn run(source: &str, success: bool, diagnostic: &str) {
    run_case(source, success, diagnostic, "conversation");
}

fn run_case(source: &str, success: bool, diagnostic: &str, case: &str) {
    let dir = tempfile::tempdir().unwrap();
    let peer = dir.path().join("peer");
    fs::write(&peer, include_bytes!("peer.py")).unwrap();
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o755)).unwrap();
    let script = dir.path().join("workflow.js");
    fs::write(&script, source).unwrap();
    let evidence = dir.path().join("evidence");
    let output = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"))
        .env("RPC_TEST_CASE", case)
        .arg("js")
        .arg(&script)
        .arg("--async")
        .arg("--due")
        .arg(&peer)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--evidence")
        .arg(&evidence)
        .args(["--timeout-ms", "500"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.success(), success, "{stderr}");
    if !success {
        assert!(stderr.contains(diagnostic), "{stderr}");
    }
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["settled"], true);
    assert_eq!(receipt["forced_kill"], false);
}

#[test]
fn awaited_promises_and_cooperative_sleep_finish_normally() {
    run(
        "Due.run(async scope=>{await scope.sleep(1); await Promise.resolve(3);});",
        true,
        "",
    );
}

#[test]
fn discarded_rejection_cannot_become_success() {
    run(
        "Due.run(async ()=>{Promise.reject(Error('discarded failure'));});",
        false,
        "discarded failure",
    );
}

#[test]
fn awaited_rejection_can_be_handled() {
    run(
        "Due.run(async ()=>{try {await Promise.reject(Error('handled'));} catch(e) {}});",
        true,
        "",
    );
}

#[test]
fn unresolved_detached_promise_is_reported() {
    run(
        "Due.run(async ()=>{new Promise(()=>{});});",
        false,
        "unsettled Promises",
    );
}

#[test]
fn unjoined_managed_timer_is_cancelled_and_fails_scope() {
    run(
        "Due.run(async scope=>{scope.sleep(30000);});",
        false,
        "unjoined work",
    );
}

#[test]
fn microtask_loop_cannot_escape_the_host_deadline() {
    run(
        "Due.run(async ()=>{for (;;) await Promise.resolve();});",
        false,
        "deadline",
    );
}

#[test]
fn terminal_turn_still_requires_an_explicit_wait() {
    run(
        "Due.run(async scope=>{const w=await scope.open({provider:'codex'}); await w.start('task');});",
        false,
        "active turn",
    );
}

#[test]
fn callback_cannot_detach_managed_work_even_with_a_rejection_handler() {
    run_case(
        "Due.run(async scope=>{const w=await scope.open({provider:'codex'}, {onInteraction(item,signal){scope.sleep(30000,signal).catch(()=>{}); return {decision:'decline'};}}); await w.turn('task');});",
        false,
        "permission callback returned with unjoined work",
        "permission",
    );
}
