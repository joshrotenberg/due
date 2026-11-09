#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn fake_provider(directory: &Path) -> std::path::PathBuf {
    fs::create_dir(directory).unwrap();
    fs::write(
        directory.join("native"),
        include_bytes!("../../../crates/due-core/tests/fixtures/session-agent.sh"),
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
fn independent_application_runs_both_providers_and_replays_their_sessions() {
    let workspace = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_due-core-compare"))
        .current_dir(workspace.path())
        .env(
            "COMPARE_CLAUDE_BINARY",
            fake_provider(&workspace.path().join("claude")),
        )
        .env(
            "COMPARE_CODEX_BINARY",
            fake_provider(&workspace.path().join("codex")),
        )
        .arg("fixture prompt")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    for provider in ["claude", "codex"] {
        for turn in [1, 2] {
            assert!(
                stdout
                    .lines()
                    .any(|line| line == format!("{provider} turn {turn}: turn {turn}")),
                "{stdout}"
            );
        }
        let prefix = format!("{provider} replayed ");
        let count: usize = stdout
            .lines()
            .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix(" session items"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(count >= 7, "{stdout}");
        assert!(workspace.path().join(provider).join("exited").is_file());
    }
}
