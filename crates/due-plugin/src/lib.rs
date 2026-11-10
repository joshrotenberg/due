//! Package and install the local Due delegate plugin.

use std::{
    fs::{self, OpenOptions},
    future::Future,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

const MARKER: &str = ".due-install.json";
const LOCK: &str = ".due-init.lock";
const MARKETPLACE: &str = "due-local";
const MANIFEST: &str = include_str!("../../../plugin/due/plugin.json");
const SKILL: &[u8] = include_bytes!("../../../plugin/due/skills/delegate/SKILL.md");
const HOST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: u64 = 64 * 1024;
const KILLED_OUTPUT_GRACE: Duration = Duration::from_secs(1);

/// Native hosts whose local plugin registration is managed by the installer.
#[derive(Debug, Clone, Copy)]
pub enum Host {
    /// Claude Code and Codex.
    Both,
    /// Claude Code only.
    Claude,
    /// Codex only.
    Codex,
}

/// Inputs for the bundled delegate plugin installation.
pub struct InstallOptions {
    /// Preview the plan without writes or native host commands.
    pub dry_run: bool,
    /// Select the hosts to configure.
    pub host: Host,
    /// Override the managed marketplace directory.
    pub install_dir: Option<PathBuf>,
    /// Override the Claude Code executable.
    pub claude_bin: Option<PathBuf>,
    /// Override the Codex executable.
    pub codex_bin: Option<PathBuf>,
    /// Path to the Due executable represented by the setup report.
    pub due_binary: PathBuf,
    /// Version of the Due executable represented by the setup report.
    pub cli_version: String,
}

/// Native command role in a setup plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Check existing marketplace registrations.
    InspectMarketplace,
    /// Register the managed local marketplace.
    AddMarketplace,
    /// Check whether the delegate plugin is installed.
    InspectPlugins,
    /// Install the delegate plugin.
    Install,
    /// Refresh an installed delegate plugin.
    Update,
}

/// One native command in the installation plan.
#[derive(Debug, Clone, Serialize)]
pub struct HostCommand {
    /// Role of this step.
    pub action: Action,
    /// Selected host name.
    pub host: String,
    /// Resolved native executable.
    pub executable: PathBuf,
    /// Arguments passed without a shell.
    pub args: Vec<String>,
}

/// The files and host commands selected during planning.
#[derive(Serialize)]
pub struct Plan {
    ready: bool,
    /// Managed marketplace directory.
    pub install_dir: PathBuf,
    binary: PathBuf,
    /// Planned file paths.
    pub files: Vec<PathBuf>,
    /// Candidate native commands in execution order.
    pub commands: Vec<HostCommand>,
    /// Preflight diagnostics.
    pub issues: Vec<String>,
}

/// Observed result of a native host command.
#[derive(Serialize)]
pub struct Step {
    /// Command that ran.
    pub command: HostCommand,
    success: bool,
    cancelled: bool,
    exit_code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    /// Diagnostic, if the command failed.
    pub error: Option<String>,
}

/// Versioned setup receipt returned for dry runs and applications.
#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    cli_version: String,
    /// Setup outcome used by the CLI and JSON receipt.
    pub status: &'static str,
    /// Prepared plan, if one could be built.
    pub plan: Option<Plan>,
    files_written: Vec<PathBuf>,
    completed_steps: Vec<Step>,
    skipped_steps: Vec<HostCommand>,
    /// Failed native step, if execution reached one.
    pub failed_step: Option<Step>,
    /// Setup diagnostic, if available.
    pub error: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    format_version: u32,
    managed_by: String,
    cli_version: String,
}

type Result<T> = std::result::Result<T, String>;

fn json_bytes(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("bundled serializable values");
    bytes.push(b'\n');
    bytes
}

fn package() -> Vec<(&'static str, Vec<u8>)> {
    let portable: Value = serde_json::from_str(MANIFEST).expect("bundled manifest");
    let native = json!({
        "name": portable["name"], "version": portable["version"],
        "description": portable["description"], "author": portable["author"],
        "license": "MIT OR Apache-2.0"
    });
    let mut codex = native.clone();
    codex["skills"] = json!("./skills/");
    codex["interface"] = portable["extensions"]["com.openai"]["interface"].clone();
    vec![
        ("plugin/due/plugin.json", MANIFEST.as_bytes().to_vec()),
        (
            "plugin/due/README.md",
            include_bytes!("../../../plugin/due/README.md").to_vec(),
        ),
        ("plugin/due/skills/delegate/SKILL.md", SKILL.to_vec()),
        ("plugin/due/.claude-plugin/plugin.json", json_bytes(&native)),
        ("plugin/due/.codex-plugin/plugin.json", json_bytes(&codex)),
        (
            ".agents/plugins/marketplace.json",
            json_bytes(&json!({
                "name":MARKETPLACE,"interface":{"displayName":"Due local"},
                "plugins":[{"name":"due","source":{"source":"local","path":"./plugin/due"},
                    "policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"},
                    "category":"Developer Tools"}]
            })),
        ),
        (
            ".claude-plugin/marketplace.json",
            json_bytes(&json!({
                "name":MARKETPLACE,"owner":{"name":"Josh Rotenberg"},
            "description":"The delegate plugin embedded in the local Due executable",
                "plugins":[{"name":"due","source":"./plugin/due","version":portable["version"],
                    "description":portable["description"]}]
            })),
        ),
    ]
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("install directory escapes the filesystem root".into());
                }
            }
            _ => normalized.push(part.as_os_str()),
        }
    }
    if normalized.parent().is_none() || normalized.to_str().is_none() {
        return Err("install directory must be a non-root UTF-8 path".into());
    }
    Ok(normalized)
}

fn directory(args: &InstallOptions) -> Result<PathBuf> {
    if let Some(path) = &args.install_dir {
        return absolute(path);
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("XDG_DATA_HOME must be absolute; use --install-dir instead".into());
        }
        return absolute(&path.join("due"));
    }
    std::env::var_os("HOME")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "HOME is unset; use --install-dir".to_owned())
        .and_then(|home| absolute(&PathBuf::from(home).join(".local/share/due")))
}

fn regular(path: &Path) -> Result<()> {
    let info = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !info.is_file() || info.file_type().is_symlink() {
        return Err(format!("not a regular file: {}", path.display()));
    }
    Ok(())
}

fn validate_tree(root: &Path, locked: bool) -> Result<()> {
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
        Ok(info) if !info.is_dir() || info.file_type().is_symlink() => {
            return Err(format!(
                "install directory must be a real directory: {}",
                root.display()
            ));
        }
        _ => {}
    }
    let marker = root.join(MARKER);
    let mut entries = fs::read_dir(root).map_err(|e| e.to_string())?;
    if entries.next().is_none() {
        return Ok(());
    }
    // A newly created root contains only our lock before its first marker write.
    if locked
        && !marker.try_exists().map_err(|e| e.to_string())?
        && fs::read_dir(root)
            .map_err(|e| e.to_string())?
            .all(|entry| entry.is_ok_and(|e| e.file_name() == LOCK))
    {
        return Ok(());
    }
    regular(&marker).map_err(|_| {
        format!(
            "refusing unmanaged install directory {}; choose an empty directory",
            root.display()
        )
    })?;
    let mut bytes = Vec::new();
    fs::File::open(&marker)
        .map_err(|e| e.to_string())?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let metadata: Metadata =
        serde_json::from_slice(&bytes).map_err(|_| "invalid Due install metadata".to_owned())?;
    if bytes.len() > 16 * 1024 || metadata.format_version != 1 || metadata.managed_by != "due" {
        return Err("unrecognized Due install metadata".into());
    }
    let mut allowed: Vec<_> = package()
        .into_iter()
        .map(|(name, _)| root.join(name))
        .collect();
    allowed.extend([marker, root.join(LOCK)]);
    validate_entries(root, &allowed)
}

fn validate_entries(dir: &Path, files: &[PathBuf]) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "refusing symlink in managed package: {}",
                path.display()
            ));
        }
        if kind.is_dir() && files.iter().any(|f| f.starts_with(&path)) {
            validate_entries(&path, files)?;
        } else if !kind.is_file() || !files.contains(&path) {
            return Err(format!(
                "refusing unexpected package entry: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn plan(args: &InstallOptions) -> Result<Plan> {
    let root = directory(args)?;
    let current_binary = args.due_binary.clone();
    if current_binary.to_str().is_none() {
        return Err("current executable path must be valid UTF-8".into());
    }
    let mut plan = Plan {
        ready: true,
        install_dir: root.clone(),
        binary: current_binary,
        files: package()
            .iter()
            .map(|(name, _)| root.join(name))
            .chain([root.join(MARKER)])
            .collect(),
        commands: Vec::new(),
        issues: Vec::new(),
    };
    if let Err(error) = validate_tree(&root, false) {
        plan.issues.push(error);
    }
    for (name, override_bin) in [("claude", &args.claude_bin), ("codex", &args.codex_bin)] {
        if !matches!(
            (args.host, name),
            (Host::Both, _) | (Host::Claude, "claude") | (Host::Codex, "codex")
        ) {
            continue;
        }
        let requested = override_bin.as_deref().unwrap_or(Path::new(name));
        if requested.to_str().is_none() {
            return Err("host executable path must be valid UTF-8".into());
        }
        let binary = match which::which(requested) {
            Ok(path) => fs::canonicalize(path).map_err(|e| e.to_string())?,
            Err(_) => {
                plan.issues.push(format!(
                    "{name} CLI is unavailable: {}; install it or select --host",
                    requested.display()
                ));
                requested.to_owned()
            }
        };
        if binary.to_str().is_none() {
            return Err("resolved host executable path must be valid UTF-8".into());
        }
        let root_text = root.to_str().expect("validated UTF-8");
        let mut add = vec!["plugin", "marketplace", "add", root_text, "--json"];
        let mut install = vec![
            "plugin",
            if name == "claude" { "install" } else { "add" },
            "due@due-local",
            "--json",
        ];
        if name == "claude" {
            add.extend(["--scope", "user"]);
            install.extend(["--scope", "user"]);
        }
        let mut steps = vec![
            (
                Action::InspectMarketplace,
                vec!["plugin", "marketplace", "list", "--json"],
            ),
            (Action::AddMarketplace, add),
        ];
        if name == "claude" {
            steps.push((Action::InspectPlugins, vec!["plugin", "list", "--json"]));
        }
        steps.push((Action::Install, install));
        if name == "claude" {
            steps.push((
                Action::Update,
                vec![
                    "plugin",
                    "update",
                    "due@due-local",
                    "--scope",
                    "user",
                    "--json",
                ],
            ));
        }
        for (action, args) in steps {
            plan.commands.push(HostCommand {
                action,
                host: name.into(),
                executable: binary.clone(),
                args: args.into_iter().map(str::to_owned).collect(),
            });
        }
    }
    plan.ready = plan.issues.is_empty();
    Ok(plan)
}

struct InstallLock(PathBuf);
impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().expect("package path has parent");
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn materialize(plan: &Plan, cli_version: &str, written: &mut Vec<PathBuf>) -> Result<InstallLock> {
    validate_tree(&plan.install_dir, false)?;
    fs::create_dir_all(&plan.install_dir).map_err(|e| e.to_string())?;
    let lock_path = plan.install_dir.join(LOCK);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "cannot acquire install lock {}: {e}; an active or interrupted init may own it",
                lock_path.display()
            )
        })?;
    let lock = InstallLock(lock_path);
    validate_tree(&plan.install_dir, true)?;
    let marker = plan.install_dir.join(MARKER);
    atomic_write(
        &marker,
        &json_bytes(&Metadata {
            format_version: 1,
            managed_by: "due".into(),
            cli_version: cli_version.into(),
        }),
    )?;
    written.push(marker);
    for (name, bytes) in package() {
        validate_tree(&plan.install_dir, true)?;
        let path = plan.install_dir.join(name);
        atomic_write(&path, &bytes)?;
        written.push(path);
    }
    Ok(lock)
}

async fn bounded_output(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    bytes: &mut Vec<u8>,
) -> io::Result<()> {
    let mut chunk = [0; 8192];
    loop {
        let capacity = (MAX_OUTPUT as usize + 1 - bytes.len()).min(chunk.len());
        let count = reader.read(&mut chunk[..capacity]).await?;
        if count == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() as u64 > MAX_OUTPUT {
            return Err(io::Error::other("host output exceeded 64 KiB"));
        }
    }
}

/// Collects output still buffered after a kill. A process outside the owned
/// group can hold a pipe open, so EOF is not guaranteed and `grace` bounds the
/// wait; bytes read before it expires are kept.
async fn drain_killed_output(
    stdout: impl tokio::io::AsyncRead + Unpin,
    stdout_bytes: &mut Vec<u8>,
    stderr: impl tokio::io::AsyncRead + Unpin,
    stderr_bytes: &mut Vec<u8>,
    grace: Duration,
) {
    let _ = tokio::time::timeout(grace, async {
        tokio::join!(
            bounded_output(stdout, stdout_bytes),
            bounded_output(stderr, stderr_bytes)
        )
    })
    .await;
}

async fn run_host(
    command: &HostCommand,
    limit: Duration,
    cancel: impl std::future::Future<Output = ()>,
) -> Step {
    let mut result = Step {
        command: command.clone(),
        success: false,
        cancelled: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        error: None,
    };
    let mut process = tokio::process::Command::new(&command.executable);
    process
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    process.process_group(0);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            result.error = Some(error.to_string());
            return result;
        }
    };
    // The attempt below only borrows the readers, so losing the select cannot discard the pipes.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let attempt = tokio::select! {
        biased;
        _ = cancel => None,
        attempt = tokio::time::timeout(limit, async {
            tokio::try_join!(bounded_output(&mut stdout, &mut stdout_bytes), bounded_output(&mut stderr, &mut stderr_bytes))?;
            child.wait().await
        }) => Some(attempt),
    };
    match attempt {
        Some(Ok(Ok(status))) => {
            result.success = status.success();
            result.exit_code = status.code();
            if !result.success {
                result.error = Some("native plugin registration failed".into());
            }
        }
        failure => {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // Signal only the process group created for this setup command.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), child.kill()).await;
            drain_killed_output(
                stdout,
                &mut stdout_bytes,
                stderr,
                &mut stderr_bytes,
                KILLED_OUTPUT_GRACE,
            )
            .await;
            result.error = Some(match failure {
                Some(Err(_)) => {
                    format!("native setup command exceeded {} seconds", limit.as_secs())
                }
                Some(Ok(Err(error))) => error.to_string(),
                None => {
                    result.cancelled = true;
                    "native setup interrupted; partial effects remain".into()
                }
                _ => unreachable!(),
            });
        }
    }
    result.stdout_truncated = stdout_bytes.len() as u64 > MAX_OUTPUT;
    result.stderr_truncated = stderr_bytes.len() as u64 > MAX_OUTPUT;
    result.stdout =
        String::from_utf8_lossy(&stdout_bytes[..stdout_bytes.len().min(MAX_OUTPUT as usize)])
            .into_owned();
    result.stderr =
        String::from_utf8_lossy(&stderr_bytes[..stderr_bytes.len().min(MAX_OUTPUT as usize)])
            .into_owned();
    result
}

// Apply inspects only native CLI JSON; dry-run never starts even these read-only commands.
fn registered(step: &Step, root: &Path) -> Result<bool> {
    let value: Value = serde_json::from_str(&step.stdout)
        .map_err(|_| "native marketplace listing was not JSON".to_owned())?;
    let entries = if step.command.host == "claude" {
        value.as_array()
    } else {
        value.get("marketplaces").and_then(Value::as_array)
    }
    .ok_or_else(|| "unsupported native marketplace listing format".to_owned())?;
    let entries: Vec<_> = entries
        .iter()
        .filter(|v| v["name"] == MARKETPLACE)
        .collect();
    if entries.is_empty() {
        return Ok(false);
    }
    if entries.len() != 1 {
        return Err(
            "multiple due-local marketplaces; resolve the host registration before re-running init"
                .into(),
        );
    }
    let entry = entries[0];
    let path = entry[if step.command.host == "claude" {
        "path"
    } else {
        "root"
    }]
    .as_str()
    .ok_or_else(|| "due-local marketplace has no local root".to_owned())?;
    if step.command.host == "claude" && entry["source"] != "directory" {
        return Err("due-local already names a different marketplace source".into());
    }
    if fs::canonicalize(path).ok() != Some(fs::canonicalize(root).map_err(|e| e.to_string())?) {
        return Err("due-local is registered at another location; inspect or remove that native marketplace before re-running init".into());
    }
    Ok(true)
}

fn claude_installed(step: &Step) -> Result<bool> {
    let value: Value = serde_json::from_str(&step.stdout)
        .map_err(|_| "native plugin listing was not JSON".to_owned())?;
    let entries = value
        .as_array()
        .ok_or_else(|| "unsupported native plugin listing format".to_owned())?;
    Ok(entries
        .iter()
        .any(|entry| entry["id"] == "due@due-local" && entry["scope"] == "user"))
}

impl Report {
    /// Build a setup receipt for an error encountered before planning.
    pub fn not_ready(cli_version: &str, error: Option<String>) -> Self {
        Self {
            schema_version: 1,
            cli_version: cli_version.into(),
            status: "not_ready",
            plan: None,
            files_written: Vec::new(),
            completed_steps: Vec::new(),
            skipped_steps: Vec::new(),
            failed_step: None,
            error,
        }
    }
}

/// Preview or install the managed plugin and return the versioned setup report.
///
/// The cancellation factory is called after successful planning and before
/// writes. A dry run never calls it. When cancellation resolves, the installer
/// settles the active host command before returning.
pub async fn install<F>(
    args: &InstallOptions,
    cancellation: impl FnOnce() -> io::Result<F>,
) -> Report
where
    F: Future<Output = ()>,
{
    let mut report = Report::not_ready(&args.cli_version, None);
    report.plan = match plan(args) {
        Ok(plan) => Some(plan),
        Err(error) => {
            report.error = Some(error);
            return report;
        }
    };
    let plan = report.plan.as_ref().expect("prepared plan");
    if !plan.ready {
        return report;
    }
    if args.dry_run {
        report.status = "planned";
        return report;
    }
    let cancel = match cancellation() {
        Ok(cancel) => cancel,
        Err(error) => {
            report.error = Some(format!("register setup cancellation: {error}"));
            return report;
        }
    };
    tokio::pin!(cancel);
    let _lock = match materialize(plan, &args.cli_version, &mut report.files_written) {
        Ok(lock) => lock,
        Err(error) => {
            report.status = "partial_install";
            report.error = Some(error);
            return report;
        }
    };
    let mut marketplace_present = false;
    let mut plugin_present = false;
    for command in &plan.commands {
        if (command.action == Action::AddMarketplace && marketplace_present)
            || (command.host == "claude" && command.action == Action::Install && plugin_present)
            || (command.action == Action::Update && !plugin_present)
        {
            report.skipped_steps.push(command.clone());
            continue;
        }
        let mut step = run_host(command, HOST_TIMEOUT, cancel.as_mut()).await;
        if step.success {
            let observation = match command.action {
                Action::InspectMarketplace => registered(&step, &plan.install_dir)
                    .map(|present| marketplace_present = present),
                Action::InspectPlugins => {
                    claude_installed(&step).map(|present| plugin_present = present)
                }
                _ => Ok(()),
            };
            if let Err(error) = observation {
                step.success = false;
                step.error = Some(error);
            }
        }
        if !step.success {
            report.status = if step.cancelled {
                "cancelled"
            } else {
                "partial_install"
            };
            report.error = Some("Package files and earlier successful host steps remain applied. Inspect the failed step before re-running init; host policy and credentials were not changed by Due.".into());
            report.failed_step = Some(step);
            return report;
        }
        report.completed_steps.push(step);
    }
    report.status = "installed";
    report
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn shell(script: &str, extra: &[&str]) -> HostCommand {
        HostCommand {
            action: Action::Install,
            host: "synthetic".into(),
            executable: "/bin/sh".into(),
            args: [vec!["-c", script, "setup-fixture"], extra.to_vec()]
                .concat()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    #[tokio::test]
    async fn native_stdout_and_stderr_are_captured_separately_on_failure() {
        let step = run_host(
            &shell("printf 'partial setup'; printf 'denied' >&2; exit 7", &[]),
            Duration::from_secs(2),
            std::future::pending(),
        )
        .await;
        assert!(!step.success);
        assert_eq!(step.exit_code, Some(7));
        assert_eq!(step.stdout, "partial setup");
        assert_eq!(step.stderr, "denied");
    }

    #[tokio::test]
    async fn noisy_host_is_bounded_and_stopped() {
        let step = run_host(
            &shell("exec /usr/bin/yes synthetic", &[]),
            Duration::from_secs(2),
            std::future::pending(),
        )
        .await;
        assert!(!step.success);
        assert!(step.error.unwrap().contains("64 KiB"));
        assert!(step.stdout_truncated);
        assert!(!step.stdout.is_empty());
        assert!(step.stdout.len() <= MAX_OUTPUT as usize);
    }

    #[tokio::test]
    async fn cancellation_captures_output_written_before_any_pipe_read() {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let watched = ready.clone();
        let step = run_host(
            &shell(
                "printf 'setup started'; printf 'awaiting native setup' >&2; : > \"$1\"; exec /bin/sleep 30",
                &[ready.to_str().unwrap()],
            ),
            Duration::from_secs(30),
            async move {
                // The ready file follows both markers. Blocking the thread without yielding to
                // the runtime resolves the biased select before its read branch is ever polled.
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while !watched.exists() {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "fixture never wrote its markers"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            },
        )
        .await;
        assert!(!step.success);
        assert!(step.cancelled);
        assert!(step.error.unwrap().contains("interrupted"));
        assert_eq!(step.stdout, "setup started");
        assert_eq!(step.stderr, "awaiting native setup");
        assert!(!step.stdout_truncated);
        assert!(!step.stderr_truncated);
    }

    #[tokio::test]
    async fn killed_output_drain_is_bounded_when_pipes_stay_open() {
        use tokio::io::AsyncWriteExt;
        let (mut stdout_writer, stdout_reader) = tokio::io::duplex(1024);
        let (mut stderr_writer, stderr_reader) = tokio::io::duplex(1024);
        stdout_writer.write_all(b"setup started").await.unwrap();
        stderr_writer.write_all(b"awaiting").await.unwrap();
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        // The writers stay alive across the drain, so neither reader can see EOF.
        tokio::time::timeout(
            Duration::from_secs(5),
            drain_killed_output(
                stdout_reader,
                &mut stdout_bytes,
                stderr_reader,
                &mut stderr_bytes,
                Duration::from_millis(50),
            ),
        )
        .await
        .expect("drain must not wait for EOF");
        assert_eq!(stdout_bytes, b"setup started");
        assert_eq!(stderr_bytes, b"awaiting");
        drop((stdout_writer, stderr_writer));
    }

    #[tokio::test]
    async fn setup_deadline_kills_only_its_owned_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let pids = temp.path().join("pids");
        let step = run_host(&shell(
            "printf 'setup started'; printf 'awaiting native setup' >&2; printf '%s\\n' \"$$\" > \"$1\"; /bin/sleep 30 & printf '%s\\n' \"$!\" >> \"$1\"; wait",
            &[pids.to_str().unwrap()],
        ), Duration::from_millis(250), std::future::pending()).await;
        assert!(!step.success);
        assert!(step.error.unwrap().contains("exceeded"));
        assert_eq!(step.stdout, "setup started");
        assert_eq!(step.stderr, "awaiting native setup");
        for pid in fs::read_to_string(pids).unwrap().lines() {
            let mut stopped = false;
            for _ in 0..100 {
                let output = std::process::Command::new("/bin/ps")
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
                if state.trim().is_empty() || state.trim().starts_with('Z') {
                    stopped = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(stopped, "native setup process {pid} still executing");
        }
    }
}
