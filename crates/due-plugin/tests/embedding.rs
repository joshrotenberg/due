#![cfg(unix)]

use std::{future::Pending, io, path::Path};

use due_plugin::{Host, InstallOptions, install};
use serde_json::json;

#[tokio::test]
async fn host_can_plan_without_the_due_executable_or_host_registration() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("managed");
    let due_binary = Path::new("/example/bin/due");
    let options = InstallOptions {
        dry_run: true,
        host: Host::Claude,
        install_dir: Some(target.clone()),
        claude_bin: Some("/bin/sh".into()),
        codex_bin: None,
        due_binary: due_binary.into(),
        cli_version: "test-client-version".into(),
    };

    let report = install(&options, || -> io::Result<Pending<()>> {
        panic!("dry run must not register cancellation");
    })
    .await;
    let receipt = serde_json::to_value(report).unwrap();
    assert_eq!(receipt["status"], "planned", "{receipt}");
    assert_eq!(receipt["schema_version"], 1);
    assert_eq!(receipt["cli_version"], "test-client-version");
    assert_eq!(receipt["plan"]["binary"], json!(due_binary));
    assert_eq!(receipt["files_written"], json!([]));
    assert_eq!(receipt["completed_steps"], json!([]));
    assert!(!target.exists());
}
