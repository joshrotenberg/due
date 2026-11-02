//! Lua-specific scheduler regressions; shared behavior is tested cross-language.
#![cfg(all(unix, feature = "lua"))]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

fn run(case: &str, source: &str, success: bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    let peer = dir.path().join("peer");
    fs::write(
        &peer,
        include_str!("peer.py")
            .replace(
                "{\"interrupted\": True}",
                "{\"interrupt_requested\": True, \"turn_id\": params[\"turn_id\"]}",
            )
            .replace(
                "    elif method == \"turn.start\":\n",
                concat!(
                    "    elif method == \"turn.start\":\n",
                    "        if CASE == \"rejected_start\" and params[\"prompt\"] == \"invalid\":\n",
                    "            send({\"jsonrpc\": \"2.0\", \"id\": request[\"id\"], \"error\": {\n",
                    "                \"code\": -32602, \"message\": \"fixture rejected start\"}})\n",
                    "            continue\n",
                ),
            )
            .replace(
                "    if method == \"session.open\":\n",
                concat!(
                    "    if method == \"session.open\":\n",
                    "        if CASE == \"rejected_open\":\n",
                    "            send({\"jsonrpc\": \"2.0\", \"id\": request[\"id\"], \"error\": {\n",
                    "                \"code\": -32602, \"message\": \"fixture rejected open\"}})\n",
                    "            continue\n",
                ),
            )
            .replace(
                "    elif method == \"turn.interrupt\":\n",
                concat!(
                    "    elif method == \"turn.interrupt\":\n",
                    "        if CASE == \"rejected_interrupt\" and not pending.get(\"interrupt_rejected\"):\n",
                    "            pending[\"interrupt_rejected\"] = True\n",
                    "            send({\"jsonrpc\": \"2.0\", \"id\": request[\"id\"], \"error\": {\n",
                    "                \"code\": -32602, \"message\": \"fixture rejected interrupt\"}})\n",
                    "            continue\n",
                ),
            )
            .replace(
                "CASE in (\"permission\", \"interrupt\")",
                "CASE in (\"permission\", \"interrupt\", \"rejected_interrupt\", \"manual_reply_timeout\")",
            )
            .replace(
                "    elif method == \"interaction.reply\":\n",
                concat!(
                    "    elif method == \"interaction.reply\":\n",
                    "        if CASE == \"manual_reply_timeout\":\n",
                    "            continue\n",
                ),
            ),
    )
    .unwrap();
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o700)).unwrap();
    let script = dir.path().join("workflow.lua");
    fs::write(&script, source).unwrap();
    let evidence = dir.path().join("evidence");
    let output = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"))
        .arg("lua")
        .arg(script)
        .arg("--async")
        .arg("--due")
        .arg(peer)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--evidence")
        .arg(&evidence)
        .args(["--timeout-ms", "5000"])
        .env("RPC_TEST_CASE", case)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.success(), success, "{stderr}");
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("workflow.json")).unwrap()).unwrap();
    assert_eq!(
        receipt["status"],
        if success { "succeeded" } else { "failed" },
        "{receipt}"
    );
    assert!(
        stderr.contains("FIXTURE_EOF"),
        "RPC child was not settled: {stderr}"
    );
    stderr
}

#[test]
fn lua_coroutine_verifies_and_continues_without_losing_private_result_state() {
    run(
        "conversation",
        r#"
Due.run(function(scope)
  local worker=scope:open({provider='codex'},{onItem=function(item)
    if item.kind=='turn_finished' then item.data.status='tampered' end
  end})
  assert(worker:turn('first').status=='succeeded')
  local check=scope:exec({program='/usr/bin/printf',args=json.array({'verified'})})
  assert(check.status=='succeeded' and check.stdout=='verified')
  local rejected=scope:exec({program='/bin/sh',args=json.array({'-c','exit 7'})})
  assert(rejected.status=='failed' and rejected.exit_code==7)
  assert(worker:turn('second').status=='succeeded')
end)
"#,
        true,
    );
}

#[test]
fn lua_permission_callback_can_yield_to_sleep_and_verifier() {
    run(
        "permission",
        r#"
Due.run(function(scope)
  local handled=0
  local worker=scope:open({provider='codex'},{onInteraction=function(item,signal)
    handled=handled+1
    scope:sleep(10,signal)
    local check=scope:exec({program='/usr/bin/printf',args=json.array({'decision'})},signal)
    assert(check.stdout=='decision')
    return {decision='decline'}
  end})
  assert(worker:turn('permission').status=='succeeded')
  assert(handled==1)
end)
"#,
        true,
    );
}

#[test]
fn lua_retired_permission_joins_cancelled_checker_and_keeps_worker_reusable() {
    run(
        "interrupt",
        r#"
Due.run(function(scope)
  local cancelled=false
  local worker=scope:open({provider='codex'},{onInteraction=function(item,signal)
    local ok,e=pcall(function()
      scope:exec({program='/bin/sleep',args=json.array({'30'})},signal)
    end)
    assert(not ok and signal.aborted and e==signal.reason)
    cancelled=true
    return {decision='decline'}
  end})
  local turn=worker:start('permission')
  scope:sleep(30)
  worker:interrupt(turn)
  assert(worker:wait(turn).status=='failed')
  scope:sleep(300)
  assert(cancelled)
  assert(worker:turn('next').status=='succeeded')
end)
"#,
        true,
    );
}

#[test]
fn lua_nil_and_false_task_errors_cannot_become_success() {
    for value in ["nil", "false"] {
        run(
            "conversation",
            &format!("Due.run(function(scope) error({value}) end)"),
            false,
        );
    }
}

#[test]
fn lua_async_mode_rejects_raw_coroutines_and_observer_waits() {
    let error = run(
        "conversation",
        "Due.run(function(scope) coroutine.create(function() end) end)",
        false,
    );
    assert!(error.contains("nil value"), "{error}");
    let error = run(
        "conversation",
        "Due.run(function(scope) scope:open({provider='codex'},{onItem=function() scope:sleep(1) end}) end)",
        false,
    );
    assert!(error.contains("managed task"), "{error}");
}

#[test]
fn lua_returning_with_unconsumed_turn_is_not_success() {
    let error = run(
        "conversation",
        "Due.run(function(scope) local worker=scope:open({provider='codex'}); worker:start('next') end)",
        false,
    );
    assert!(error.contains("active turn"), "{error}");
}

#[test]
fn lua_rejected_start_releases_its_turn_reservation() {
    run(
        "rejected_start",
        r#"
Due.run(function(scope)
  local worker=scope:open({provider='codex'})
  local ok,problem=pcall(function() worker:start('invalid') end)
  assert(not ok and problem.code==-32602 and problem.message=='fixture rejected start')
  assert(worker:turn('corrected').status=='succeeded')
end)
"#,
        true,
    );
}

#[test]
fn lua_invalid_wait_timeout_does_not_consume_the_wait_slot() {
    run(
        "conversation",
        r#"
Due.run(function(scope)
  local worker=scope:open({provider='codex'})
  local turn=worker:start('next')
  local ok,problem=pcall(function() worker:wait(turn,0) end)
  assert(not ok and tostring(problem):find('Invalid turn wait timeout',1,true))
  assert(worker:wait(turn).status=='succeeded')
end)
"#,
        true,
    );
}

#[test]
fn lua_caught_open_rejection_cannot_become_workflow_success() {
    let error = run(
        "rejected_open",
        r#"
Due.run(function(scope)
  local ok,problem=pcall(function() scope:open({provider='codex'}) end)
  assert(not ok and problem.message=='fixture rejected open')
end)
"#,
        false,
    );
    assert!(error.contains("fixture rejected open"), "{error}");
}

#[test]
fn lua_rejected_interrupt_releases_its_reservation() {
    run(
        "rejected_interrupt",
        r#"
Due.run(function(scope)
  local worker=scope:open({provider='codex'},{onInteraction=function(item,signal)
    scope:sleep(500,signal)
    return {decision='decline'}
  end})
  local turn=worker:start('permission')
  local ok,problem=pcall(function() worker:interrupt(turn) end)
  assert(not ok and problem.message=='fixture rejected interrupt')
  assert(worker:interrupt(turn).interrupt_requested)
  assert(worker:wait(turn).status=='failed')
  assert(worker:turn('next').status=='succeeded')
end)
"#,
        true,
    );
}

#[test]
fn lua_caught_manual_reply_timeout_remains_sticky() {
    let error = run(
        "manual_reply_timeout",
        r#"
Due.run(function(scope)
  local interaction
  local worker=scope:open({provider='codex'},{onInteraction=function(item,signal)
    interaction=item
    scope:sleep(300000,signal)
    return {decision='decline'}
  end})
  worker:start('permission')
  scope:sleep(1)
  assert(interaction)
  local now=bridge.now_ms()
  bridge.now_ms=function() now=now+10000; return now end
  local ok,problem=pcall(function() worker:reply(interaction,{decision='decline'}) end)
  assert(not ok and tostring(problem):find('interaction.reply acknowledgement timed out',1,true))
end)
"#,
        false,
    );
    assert!(
        error.contains("interaction.reply acknowledgement timed out"),
        "{error}"
    );
}
