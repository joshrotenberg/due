use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub fn fake_provider(directory: &Path) -> PathBuf {
    fs::create_dir(directory).unwrap();
    fs::write(
        directory.join("native"),
        include_bytes!("../../../due-core/tests/fixtures/session-agent.sh"),
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
