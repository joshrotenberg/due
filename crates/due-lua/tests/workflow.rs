#![cfg(unix)]

use std::{fs, process::Command, time::Duration};

use serde_json::{Value, json};

#[path = "../../due-harness/tests/support/fake_provider.rs"]
mod support;

use support::fake_provider;

#[test]
fn lua_and_javascript_return_the_same_two_provider_workflow() {
    let lua = tempfile::tempdir().unwrap();
    let lua_root = lua.path();
    let script = lua_root.join("workflow.lua");
    fs::write(&script, include_str!("fixtures/two-provider.lua")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_due-lua"))
        .current_dir(lua_root)
        .env(
            "DUE_LUA_CLAUDE_BINARY",
            fake_provider(&lua_root.join("claude")),
        )
        .env(
            "DUE_LUA_CODEX_BINARY",
            fake_provider(&lua_root.join("codex")),
        )
        .arg(script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let mut lua_result: Value = serde_json::from_slice(&output.stdout).unwrap();

    let js = tempfile::tempdir().unwrap();
    let js_root = js.path();
    let mut js_result = due_js::run(due_js::Options {
        source: include_str!("../../due-js/tests/fixtures/two-provider.js").into(),
        args: json!({}),
        cwd: js_root.to_owned(),
        claude_binary: Some(fake_provider(&js_root.join("claude"))),
        codex_binary: Some(fake_provider(&js_root.join("codex"))),
        timeout: Duration::from_secs(10),
    })
    .unwrap();
    for (index, (provider, answer)) in [
        ("claude", "permission allowed"),
        ("codex", "permission denied"),
    ]
    .into_iter()
    .enumerate()
    {
        let row = &lua_result[index];
        assert_eq!(row["provider"], provider);
        assert_eq!(row["first"], "turn 1");
        assert_eq!(row["second"], answer);
        assert_eq!(row["sameNativeSession"], true);
        assert_eq!(row["approvals"], 1);
        // Native event delivery can vary with scheduling. Both must see items,
        // while turn results, session continuity, and approval counts must match.
        for result in [&mut lua_result, &mut js_result] {
            let items = result[index].as_object_mut().unwrap().remove("items");
            assert!(items.unwrap().as_u64().unwrap() > 0);
        }
        for root in [lua_root, js_root] {
            assert!(root.join(provider).join("exited").is_file());
        }
    }
    assert_eq!(lua_result, js_result);
}

#[test]
fn lua_parallel_failure_settles_worker_with_pending_approval() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let error = due_lua::run(due_lua::Options {
        source: r#"
Due.run(function(scope)
  local run = scope.spawn("codex", "ask")
  local after = 0
  while true do
    local page = scope.readItems(run.session, after)
    for _, item in ipairs(page.items) do
      if item.interaction ~= json.null then
        -- A caught branch failure must not allow the workflow to publish success.
        pcall(scope.parallel, function() error("fixture abort at approval") end)
        return true
      end
    end
    after = page.cursor
    if scope.wait({ run.run }, 25).condition_met then
      error("provider finished without an approval")
    end
  end
end)
"#
        .into(),
        args: json!({}),
        cwd: root.to_owned(),
        claude_binary: None,
        codex_binary: Some(fake_provider(&root.join("codex"))),
        timeout: Duration::from_secs(10),
    })
    .unwrap_err();
    assert!(error.contains("fixture abort at approval"), "{error}");
    assert!(root.join("codex").join("exited").is_file());
}

#[test]
fn lua_deadline_interrupts_a_cpu_bound_parallel_task() {
    let workspace = tempfile::tempdir().unwrap();
    let error = due_lua::run(due_lua::Options {
        source: r#"
Due.run(function(scope)
  return scope.parallel(function() while true do end end)
end)
"#
        .into(),
        args: json!({}),
        cwd: workspace.path().to_owned(),
        claude_binary: None,
        codex_binary: None,
        timeout: Duration::from_millis(250),
    })
    .unwrap_err();
    assert!(error.contains("workflow deadline exceeded"), "{error}");
}
