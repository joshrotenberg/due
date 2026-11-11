//! Execute the published client unchanged except for a short caller budget in the deadline case.
#![cfg(all(unix, feature = "rpc"))]

use serde_json::Value;
use std::{fs, os::unix::fs::symlink, path::Path, process::Output, time::Duration};
use tempfile::TempDir;
use tokio::process::Command;

const RUNNER: &str = r#"
import ast, os, sys
from pathlib import Path
tree = ast.parse(Path(sys.argv[1]).read_text())
if os.environ['DUE_EXAMPLE_SCENARIO'] == 'deadline':
    changed = False
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'TURN_WAIT_SECONDS' for t in node.targets):
            node.value = ast.Constant(value=0.1)
            changed = True
    assert changed, 'example must expose its caller-selected turn wait budget'
exec(compile(ast.fix_missing_locations(tree), '<installed-rpc-example>', 'exec'))
"#;

async fn run(scenario: &str) -> (TempDir, Output) {
    let dir = tempfile::tempdir().unwrap();
    let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rpc-example-peer.py");
    symlink(peer, dir.path().join("due")).unwrap();
    let guide = include_str!("../embedded-skills/rpc/SKILL.md");
    let source = guide
        .split("```python\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap())
        .find(|block| block.contains("class Due:"))
        .expect("published Python client");
    let script = dir.path().join("client.py");
    fs::write(&script, source).unwrap();
    let mut paths = vec![dir.path().to_owned()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut command = Command::new("python3");
    command
        .args(["-B", "-c", RUNNER])
        .arg(script)
        .current_dir(dir.path())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("DUE_EXAMPLE_SCENARIO", scenario)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .expect("published client failed to settle within the test watchdog")
        .unwrap();
    assert!(
        dir.path().join("peer-exited").exists(),
        "peer was not settled: {output:?}"
    );
    (dir, output)
}

#[tokio::test]
async fn published_python_client_fails_explicitly_on_incomplete_streams() {
    for scenario in [
        "omit-terminal",
        "omit-interaction",
        "push-drop",
        "cursor-gap",
    ] {
        let (_, output) = run(scenario).await;
        assert!(!output.status.success(), "{scenario}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("Incomplete session stream"),
            "{scenario}: {error}"
        );
        assert!(output.stdout.is_empty(), "loss must not publish acceptance");
    }
}

#[tokio::test]
async fn published_python_client_bounds_terminal_wait_separately_from_rpc_acknowledgement() {
    let (_, output) = run("deadline").await;
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("Caller turn wait deadline exceeded"),
        "{error}"
    );
}

#[tokio::test]
async fn published_python_client_preserves_success_permission_denial_and_cleanup() {
    for scenario in ["success", "denial"] {
        let (dir, output) = run(scenario).await;
        assert!(output.status.success(), "{scenario}: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "succeeded fake answer\n"
        );
        let requests: Vec<Value> = fs::read_to_string(dir.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(requests.first().unwrap()["method"], "session.open");
        assert_eq!(requests.last().unwrap()["method"], "session.close");
        let replies: Vec<_> = requests
            .iter()
            .filter(|r| r["method"] == "interaction.reply")
            .collect();
        assert_eq!(replies.len(), usize::from(scenario == "denial"));
        if let Some(reply) = replies.first() {
            assert_eq!(reply["params"]["response"]["behavior"], "deny");
        }
    }
}
