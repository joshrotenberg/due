//! Installer tests use relocated binaries, empty environments and fake hosts.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    binary: PathBuf,
    host: PathBuf,
    codex_host: PathBuf,
    calls: PathBuf,
    install: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp
            .path()
            .join(format!("due{}", std::env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_due"), &binary).unwrap();
        let host = temp.path().join("native host ' ; $(literal).claude");
        let codex_host = temp.path().join("native host ' ; $(literal).codex");
        let calls = temp.path().join("calls");
        let install = temp.path().join("managed plugin ' ; $(literal)");
        for dir in ["cwd", "tmp", "home"] {
            fs::create_dir(temp.path().join(dir)).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = r#"#!/bin/sh
printf '%s\n' "$@" >> "$DUE_INIT_CALLS"
printf -- '--step--\n' >> "$DUE_INIT_CALLS"
if [ -n "${DUE_INIT_FAIL:-}" ] && [ "$2" = "$DUE_INIT_FAIL" ]; then printf 'synthetic setup failure\n' >&2; exit 7; fi
if [ -n "${DUE_INIT_BLOCK:-}" ]; then
    printf 'native setup started'
    printf 'native setup awaiting completion' >&2
    printf '%s\n' "$$" > "$DUE_INIT_PIDS"
    /bin/sleep 30 &
    printf '%s\n' "$!" >> "$DUE_INIT_PIDS"
    : > "$DUE_INIT_READY"
    wait
fi
case "$0" in *.codex) kind=codex; marker="$DUE_INIT_MARKET_CODEX" ;; *) kind=claude; marker="$DUE_INIT_MARKET_CLAUDE" ;; esac
if [ "$2" = marketplace ]; then
    if [ "$3" = list ]; then
        if [ -n "${DUE_INIT_BAD_LIST:-}" ]; then printf '{}\n'
        elif [ -f "$marker" ] || [ -n "${DUE_INIT_CONFLICT:-}" ]; then
            root="$DUE_INIT_ROOT_JSON"
            if [ -n "${DUE_INIT_CONFLICT:-}" ]; then root='"/different/native/marketplace"'; fi
            if [ "$kind" = claude ]; then printf '[{"name":"due-local","source":"directory","path":%s}]\n' "$root"
            else printf '{"marketplaces":[{"name":"due-local","root":%s}]}\n' "$root"; fi
        elif [ "$kind" = claude ]; then printf '[]\n'; else printf '{"marketplaces":[]}\n'; fi
    else
        if [ -f "$marker" ]; then printf 'already registered\n' >&2; exit 9; fi
        : > "$marker"
        printf '{"native_setup":true}\n'
    fi
elif [ "$2" = list ]; then
    if [ -f "$DUE_INIT_PLUGIN" ]; then printf '[{"id":"due@due-local","scope":"user"}]\n'; else printf '[]\n'; fi
elif [ "$2" = install ]; then
    if [ -f "$DUE_INIT_PLUGIN" ]; then printf 'already installed\n' >&2; exit 9; fi
    : > "$DUE_INIT_PLUGIN"
    printf '{"native_setup":true}\n'
elif [ "$2" = update ]; then
    [ -f "$DUE_INIT_PLUGIN" ] || exit 9
    printf '{"updated":true}\n'
else printf '{"native_setup":true}\n'
fi
"#;
            for path in [&host, &codex_host] {
                fs::write(path, script).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        Self {
            temp,
            binary,
            host,
            codex_host,
            calls,
            install,
        }
    }

    fn command(&self) -> Command {
        self.command_with(&self.host, &self.codex_host)
    }

    fn command_with(&self, claude: &Path, codex: &Path) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(self.temp.path().join("cwd"))
            .env_clear()
            .env("PATH", "")
            .env("HOME", self.temp.path().join("home"))
            .env("TMPDIR", self.temp.path().join("tmp"))
            .env("DUE_INIT_CALLS", &self.calls)
            .env(
                "DUE_INIT_MARKET_CLAUDE",
                self.temp.path().join("market-claude"),
            )
            .env(
                "DUE_INIT_MARKET_CODEX",
                self.temp.path().join("market-codex"),
            )
            .env("DUE_INIT_PLUGIN", self.temp.path().join("installed-claude"))
            .env(
                "DUE_INIT_ROOT_JSON",
                serde_json::to_string(&self.install).unwrap(),
            )
            .args(["init", "--output", "json", "--install-dir"])
            .arg(&self.install)
            .arg("--claude-bin")
            .arg(claude)
            .arg("--codex-bin")
            .arg(codex);
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}

fn report(output: &Output, exit: i32, status: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["cli_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["status"], status);
    value
}

#[test]
fn relocated_dry_run_is_write_free_and_starts_no_host() {
    let f = Fixture::new();
    let value = report(&f.run(&["--dry-run"]), 0, "planned");
    assert_eq!(value["plan"]["ready"], true);
    assert_eq!(value["plan"]["commands"].as_array().unwrap().len(), 8);
    assert_eq!(value["plan"]["files"].as_array().unwrap().len(), 8);
    assert!(!f.install.exists());
    assert!(!f.calls.exists());
    assert_eq!(fs::read_dir(f.temp.path().join("tmp")).unwrap().count(), 0);
    assert!(value["files_written"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn standalone_apply_materializes_both_formats_and_only_native_plugin_commands() {
    let f = Fixture::new();
    let value = report(&f.run(&[]), 0, "installed");
    assert_eq!(value["completed_steps"].as_array().unwrap().len(), 7);
    assert_eq!(value["files_written"].as_array().unwrap().len(), 8);
    let plugin = f.install.join("plugin/due");
    assert_eq!(
        fs::read(plugin.join("plugin.json")).unwrap(),
        include_bytes!("../plugin/due/plugin.json")
    );
    assert_eq!(
        fs::read(plugin.join("skills/delegate/SKILL.md")).unwrap(),
        include_bytes!("../plugin/due/skills/delegate/SKILL.md")
    );
    for file in [
        "plugin.json",
        ".claude-plugin/plugin.json",
        ".codex-plugin/plugin.json",
    ] {
        let manifest: Value =
            serde_json::from_slice(&fs::read(plugin.join(file)).unwrap()).unwrap();
        assert_eq!(manifest["name"], "due");
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    }
    for path in [
        ".agents/plugins/marketplace.json",
        ".claude-plugin/marketplace.json",
    ] {
        let market: Value =
            serde_json::from_slice(&fs::read(f.install.join(path)).unwrap()).unwrap();
        assert_eq!(market["name"], "due-local");
        assert_eq!(market["plugins"][0]["name"], "due");
    }
    let commands: Vec<_> = value["completed_steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["command"]["args"].clone())
        .collect();
    assert_eq!(commands[0][0], "plugin");
    assert_eq!(commands[0][1], "marketplace");
    assert_eq!(commands[1][3], f.install.to_str().unwrap());
    assert_eq!(commands[3][1], "install");
    assert_eq!(commands[3][2], "due@due-local");
    assert_eq!(commands[6][1], "add");
    assert!(!f.install.join("bin").exists());
    assert!(!f.install.join(".due-init.lock").exists());
    assert!(!f.temp.path().join("cwd/literal").exists());
}

#[cfg(unix)]
#[test]
fn selected_host_does_not_require_or_call_the_other_host() {
    let f = Fixture::new();
    let output = f
        .command_with(&f.host, Path::new("/missing/codex"))
        .args(["--host", "claude"])
        .output()
        .unwrap();
    let value = report(&output, 0, "installed");
    assert_eq!(value["completed_steps"].as_array().unwrap().len(), 4);
    assert!(
        value["completed_steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["command"]["host"] == "claude")
    );
}

#[test]
fn missing_host_reports_not_ready_without_files_or_processes() {
    let f = Fixture::new();
    for dry in [false, true] {
        let mut cmd = f.command_with(&f.host, Path::new("/missing/codex"));
        cmd.args(["--host", "codex"]);
        if dry {
            cmd.arg("--dry-run");
        }
        let value = report(&cmd.output().unwrap(), 1, "not_ready");
        assert_eq!(value["plan"]["ready"], false);
        assert!(
            value["plan"]["issues"][0]
                .as_str()
                .unwrap()
                .contains("unavailable")
        );
        assert!(!f.install.exists());
        assert!(!f.calls.exists());
    }
}

#[cfg(unix)]
#[test]
fn failed_native_step_preserves_local_package_and_stops_before_later_hosts() {
    let f = Fixture::new();
    let value = report(
        &f.command()
            .env("DUE_INIT_FAIL", "install")
            .output()
            .unwrap(),
        1,
        "partial_install",
    );
    assert_eq!(value["completed_steps"].as_array().unwrap().len(), 3);
    assert_eq!(value["failed_step"]["exit_code"], 7);
    assert!(
        value["failed_step"]["stderr"]
            .as_str()
            .unwrap()
            .contains("synthetic setup failure")
    );
    assert_eq!(value["files_written"].as_array().unwrap().len(), 8);
    assert!(
        f.install
            .join("plugin/due/skills/delegate/SKILL.md")
            .exists()
    );
    assert_eq!(
        fs::read_to_string(&f.calls)
            .unwrap()
            .matches("--step--")
            .count(),
        4
    );
    assert!(!f.install.join(".due-init.lock").exists());
    report(&f.run(&[]), 0, "installed");
}

#[cfg(unix)]
#[test]
fn rerun_refreshes_only_owned_package_and_lock_is_exclusive() {
    let f = Fixture::new();
    report(&f.run(&[]), 0, "installed");
    let skill = f.install.join("plugin/due/skills/delegate/SKILL.md");
    fs::write(&skill, "old managed bytes").unwrap();
    let again = report(&f.run(&[]), 0, "installed");
    assert_eq!(again["completed_steps"].as_array().unwrap().len(), 5);
    assert_eq!(again["skipped_steps"].as_array().unwrap().len(), 3);
    assert!(
        again["completed_steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["command"]["action"] == "update")
    );
    assert_eq!(
        fs::read(skill).unwrap(),
        include_bytes!("../plugin/due/skills/delegate/SKILL.md")
    );
    let calls = fs::read(&f.calls).unwrap();
    fs::write(
        f.install.join(".due-init.lock"),
        "active or interrupted installer",
    )
    .unwrap();
    let value = report(&f.run(&[]), 1, "partial_install");
    assert!(value["error"].as_str().unwrap().contains("lock"));
    assert_eq!(fs::read(&f.calls).unwrap(), calls);
    assert!(f.install.join(".due-init.lock").exists());
}

#[test]
fn refuses_unmanaged_directories_and_bad_metadata_without_changes() {
    let f = Fixture::new();
    fs::create_dir(&f.install).unwrap();
    let canary = f.install.join("personal.txt");
    fs::write(&canary, "keep").unwrap();
    for marker in [
        None,
        Some("{}"),
        Some("{\"format_version\":2,\"managed_by\":\"due\",\"cli_version\":\"0.1.0\"}"),
    ] {
        if let Some(bytes) = marker {
            fs::write(f.install.join(".due-install.json"), bytes).unwrap();
        }
        report(&f.run(&[]), 1, "not_ready");
        assert_eq!(fs::read_to_string(&canary).unwrap(), "keep");
        assert!(!f.calls.exists());
    }
}

#[cfg(unix)]
#[test]
fn symlinked_roots_and_nested_assets_are_never_followed() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &f.install).unwrap();
    report(&f.run(&[]), 1, "not_ready");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    fs::remove_file(&f.install).unwrap();
    report(&f.run(&[]), 0, "installed");
    let calls = fs::read(&f.calls).unwrap();
    let asset = f.install.join("plugin/due/skills/delegate/SKILL.md");
    let canary = outside.join("canary");
    fs::write(&canary, "keep").unwrap();
    fs::remove_file(&asset).unwrap();
    symlink(&canary, &asset).unwrap();
    report(&f.run(&[]), 1, "not_ready");
    assert_eq!(fs::read_to_string(&canary).unwrap(), "keep");
    assert_eq!(fs::read(&f.calls).unwrap(), calls);
}

#[test]
fn default_directory_uses_data_home_then_home_and_rejects_root() {
    let f = Fixture::new();
    let run = |extra: &[&str], data: Option<&Path>| {
        let mut cmd = Command::new(&f.binary);
        cmd.env_clear()
            .env("PATH", "")
            .env("HOME", f.temp.path().join("home"))
            .args([
                "init",
                "--dry-run",
                "--output",
                "json",
                "--host",
                "claude",
                "--claude-bin",
            ])
            .arg(&f.host)
            .args(extra);
        if let Some(path) = data {
            cmd.env("XDG_DATA_HOME", path);
        }
        cmd.output().unwrap()
    };
    let value = report(&run(&[], None), 0, "planned");
    assert_eq!(
        value["plan"]["install_dir"],
        f.temp
            .path()
            .join("home/.local/share/due")
            .to_str()
            .unwrap()
    );
    let data = f.temp.path().join("data");
    let value = report(&run(&[], Some(&data)), 0, "planned");
    assert_eq!(
        value["plan"]["install_dir"],
        data.join("due").to_str().unwrap()
    );
    report(&run(&[], Some(Path::new("relative-data"))), 1, "not_ready");
    report(&run(&["--install-dir", "/"], None), 1, "not_ready");
    assert!(!data.exists());
}

#[test]
fn init_help_explains_scope_and_rejects_invalid_host() {
    let f = Fixture::new();
    let output = Command::new(&f.binary)
        .args(["init", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for word in [
        "--dry-run",
        "--host",
        "--install-dir",
        "--output",
        "PATH",
        "no model work",
    ] {
        assert!(text.contains(word), "missing {word}");
    }
    assert_eq!(f.run(&["--host", "unsupported"]).status.code(), Some(2));
}

#[cfg(unix)]
#[test]
fn conflicting_marketplace_or_bad_native_json_stops_before_registration() {
    for setting in ["DUE_INIT_CONFLICT", "DUE_INIT_BAD_LIST"] {
        let f = Fixture::new();
        let value = report(
            &f.command().env(setting, "1").output().unwrap(),
            1,
            "partial_install",
        );
        assert_eq!(
            value["failed_step"]["command"]["action"],
            "inspect_marketplace"
        );
        assert_eq!(value["failed_step"]["exit_code"], 0);
        assert!(value["completed_steps"].as_array().unwrap().is_empty());
        assert!(!f.temp.path().join("market-claude").exists());
        assert!(!f.temp.path().join("market-codex").exists());
        assert_eq!(
            fs::read_to_string(&f.calls)
                .unwrap()
                .matches("--step--")
                .count(),
            1
        );
    }
}

#[cfg(unix)]
#[test]
fn cancellation_reports_partial_setup_and_settles_its_native_processes() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = Fixture::new();
    let ready = f.temp.path().join("ready");
    let pids = f.temp.path().join("pids");
    let mut child = f
        .command()
        .env("DUE_INIT_BLOCK", "1")
        .env("DUE_INIT_READY", &ready)
        .env("DUE_INIT_PIDS", &pids)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        if child.try_wait().unwrap().is_some() {
            panic!("setup exited before native readiness");
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native setup was not ready");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("setup did not settle cancellation");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let value = report(&child.wait_with_output().unwrap(), 130, "cancelled");
    assert_eq!(value["failed_step"]["cancelled"], true);
    assert_eq!(value["failed_step"]["stdout"], "native setup started");
    assert_eq!(
        value["failed_step"]["stderr"],
        "native setup awaiting completion"
    );
    assert_eq!(value["files_written"].as_array().unwrap().len(), 8);
    assert!(!f.install.join(".due-init.lock").exists());
    for pid in fs::read_to_string(pids).unwrap().lines() {
        let output = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        assert!(
            output.stderr.is_empty()
                && (output.status.success() || output.status.code() == Some(1)),
            "process inspection failed: {:?}",
            output
        );
        let state = String::from_utf8_lossy(&output.stdout);
        assert!(
            state.trim().is_empty() || state.trim().starts_with('Z'),
            "process {pid} still running: {state}"
        );
    }
}

#[cfg(unix)]
#[test]
fn invalid_utf8_paths_are_rejected_before_effects() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let f = Fixture::new();
    let invalid = PathBuf::from(OsString::from_vec(b"/invalid-\xff".to_vec()));
    let value = report(
        &f.command_with(&invalid, &f.codex_host).output().unwrap(),
        1,
        "not_ready",
    );
    assert!(value["error"].as_str().unwrap().contains("UTF-8"));
    assert!(!f.install.exists());
    assert!(!f.calls.exists());
}
