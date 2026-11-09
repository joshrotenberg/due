#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde_json::{Value, json};

fn fake_provider(directory: &Path) -> PathBuf {
    fs::create_dir(directory).unwrap();
    fs::write(
        directory.join("native"),
        include_bytes!("../../due-core/tests/fixtures/session-agent.sh"),
    )
    .unwrap();
    let executable = directory.join("provider");
    fs::write(
        &executable,
        r#"#!/bin/sh
export DUE_SESSION_FIXTURE_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)"
exec /bin/bash "$DUE_SESSION_FIXTURE_DIR/native" "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    executable
}

#[test]
fn standalone_workflow_uses_both_providers_and_explicit_follow_up_approvals() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let script = root.join("workflow.js");
    fs::write(&script, include_str!("fixtures/two-provider.js")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_due-js"))
        .current_dir(root)
        .env("DUE_JS_CLAUDE_BINARY", fake_provider(&root.join("claude")))
        .env("DUE_JS_CODEX_BINARY", fake_provider(&root.join("codex")))
        .arg(script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    for (index, (provider, answer)) in [
        ("claude", "permission allowed"),
        ("codex", "permission denied"),
    ]
    .into_iter()
    .enumerate()
    {
        let row = &result[index];
        assert_eq!(row["provider"], provider);
        assert_eq!(row["first"], "turn 1");
        assert_eq!(row["second"], answer);
        assert_eq!(row["sameNativeSession"], true);
        assert_eq!(row["approvals"], 1);
        assert!(row["items"].as_u64().unwrap() > 0);
        assert!(root.join(provider).join("exited").is_file());
    }
}

#[test]
fn thrown_workflow_settles_a_worker_while_approval_is_pending() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let result = due_js::run(due_js::Options {
        source: r#"
Due.run(async scope => {
  const run = await scope.spawn('codex', 'ask');
  let after = 0;
  for (;;) {
    const page = await scope.readItems(run.session, after);
    if (page.items.some(item => item.interaction)) throw new Error('fixture abort at approval');
    after = page.cursor;
    const observed = await scope.wait([run.run], 25);
    if (observed.condition_met) throw new Error('provider finished without an approval');
  }
});
"#
        .into(),
        args: json!({}),
        cwd: root.to_owned(),
        claude_binary: None,
        codex_binary: Some(fake_provider(&root.join("codex"))),
        timeout: Duration::from_secs(10),
    });
    let error = result.unwrap_err();
    assert!(error.contains("fixture abort at approval"), "{error}");
    assert!(root.join("codex").join("exited").is_file());
}
