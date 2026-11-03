//! Promise-specific ownership rules complement the shared language scenarios.
#![cfg(all(unix, feature = "js"))]

use std::{fs, path::Path, process::Command};

const DETACHED_CALLBACK: &str = r#"
Due.run(async scope => {
  bridge.log('BEFORE_OPEN');
  const w = await scope.open({provider:'codex'}, {
    onInteraction(item, signal) {
      bridge.log('CALLBACK_ENTERED');
      scope.sleep(30000, signal).catch(error => {
        if (!signal.aborted || error !== signal.reason) throw Error('unexpected sleep rejection');
        bridge.log('CALLBACK_SLEEP_CANCELLED');
      });
      return {decision:'decline'};
    }
  });
  bridge.log('OPENED');
  await w.turn('task');
});
"#;

fn run(source: &str, success: bool, diagnostic: &str) {
    run_case(source, success, diagnostic, "conversation", 500, 0);
}

fn run_case(
    source: &str,
    success: bool,
    diagnostic: &str,
    case: &str,
    timeout_ms: u64,
    open_delay_ms: u64,
) -> String {
    let dir = tempfile::tempdir().unwrap();
    // Execute the checked-in fixture: writing executables while other tests
    // fork can leave inherited writable handles and cause ETXTBSY on Linux.
    let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/peer.py");
    let script = dir.path().join("workflow.js");
    fs::write(&script, source).unwrap();
    let evidence = dir.path().join("evidence");
    let output = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"))
        .env("RPC_TEST_CASE", case)
        .env("RPC_TEST_OPEN_DELAY_MS", open_delay_ms.to_string())
        .arg("js")
        .arg(&script)
        .arg("--async")
        .arg("--due")
        .arg(&peer)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--evidence")
        .arg(&evidence)
        .arg("--timeout-ms")
        .arg(timeout_ms.to_string())
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["settled"], true, "{stderr}\n{receipt}");
    assert_eq!(receipt["forced_kill"], false, "{stderr}\n{receipt}");
    assert_eq!(receipt["exit_code"], 0, "{stderr}\n{receipt}");
    assert!(stderr.contains("FIXTURE_EOF"), "{stderr}");
    assert_eq!(output.status.success(), success, "{stderr}");
    if !success {
        assert!(stderr.contains(diagnostic), "{stderr}");
    }
    stderr
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
    // This tests callback ownership, not an end-to-end 500 ms deadline. Leave
    // room for peer startup, but keep the budget below the detached 30 s timer.
    let stderr = run_case(
        DETACHED_CALLBACK,
        false,
        "permission callback returned with unjoined work",
        "permission",
        5000,
        1000,
    );
    assert!(stderr.contains("FIXTURE_OPEN_DELAY"), "{stderr}");
    assert!(stderr.contains("CALLBACK_ENTERED"), "{stderr}");
    assert!(stderr.contains("CALLBACK_SLEEP_CANCELLED"), "{stderr}");
    assert!(!stderr.contains("deadline"), "{stderr}");
}

#[test]
fn short_workflow_deadline_expires_before_permission_callback() {
    // The same source and delayed peer as the ownership test above: the old
    // budget expires during open, so its error cannot diagnose callback work.
    let stderr = run_case(
        DETACHED_CALLBACK,
        false,
        "workflow deadline exceeded",
        "permission",
        500,
        1000,
    );
    assert!(stderr.contains("BEFORE_OPEN"), "{stderr}");
    assert!(stderr.contains("FIXTURE_OPEN_DELAY"), "{stderr}");
    assert!(!stderr.contains("OPENED"), "{stderr}");
    assert!(!stderr.contains("CALLBACK_"), "{stderr}");
    assert!(!stderr.contains("permission callback"), "{stderr}");
}
