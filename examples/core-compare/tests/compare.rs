#![cfg(unix)]

mod support;

use std::process::Command;

use support::fake_provider;

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
