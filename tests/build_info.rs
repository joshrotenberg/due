//! `due build-info` and the compile-time source identity probe.

#[allow(dead_code)]
#[path = "../build/git_identity.rs"]
mod git_identity;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use git_identity::{Status, Tree};
use serde_json::Value;

// Match the existing relocation tests: a fork must not retain a writable
// handle to an executable while another thread tries to execute its copy.
static COPY_OR_SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn capture(command: &mut Command) -> std::io::Result<Output> {
    let child = {
        let _guard = COPY_OR_SPAWN.lock().unwrap();
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
    };
    child.wait_with_output()
}

fn probe(root: &Path, program: &OsStr) -> git_identity::Identity {
    // The probe itself starts Git children in this test process, unlike the
    // build script's separate process. They share the same copy boundary.
    let _guard = COPY_OR_SPAWN.lock().unwrap();
    git_identity::probe(root, program)
}

fn due(dir: Option<&Path>, binary: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(binary);
    command.args(args).stdin(Stdio::null());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    capture(&mut command).unwrap()
}

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_due"))
}

fn json(output: &Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn expected_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    if cfg!(feature = "core") {
        features.push("core");
    }
    if cfg!(feature = "harness") {
        features.push("harness");
    }
    if cfg!(feature = "mcp-stdio") {
        features.push("mcp-stdio");
    }
    if cfg!(feature = "rpc") {
        features.push("rpc");
    }
    features
}

#[test]
fn json_is_a_versioned_compile_time_identity() {
    let document = json(&due(None, &bin(), &["build-info", "--output", "json"]));
    let mut keys: Vec<&str> = document
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["features", "package", "schema_version", "source"]);
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["package"]["name"], "due");
    assert_eq!(document["package"]["version"], env!("CARGO_PKG_VERSION"));
    let features: Vec<&str> = document["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    // Run with --no-default-features or --features rpc to check reduced builds.
    assert_eq!(features, expected_features());

    let source = document["source"].as_object().unwrap();
    assert_eq!(source.len(), 3, "{source:?}");
    let status = source["status"].as_str().unwrap();
    let tree = source["tree"].as_str().unwrap();
    assert!(["clean", "dirty", "unknown"].contains(&tree), "{tree}");
    match status {
        "recorded" => {
            let revision = source["revision"].as_str().unwrap();
            assert!(matches!(revision.len(), 40 | 64), "{revision}");
            assert!(revision.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        "no_git_metadata" | "git_unavailable" | "not_repository_root" | "git_error" => {
            assert!(source["revision"].is_null(), "{source:?}");
            assert_eq!(tree, "unknown");
        }
        other => panic!("unexpected source status {other}"),
    }

    // No build paths or environment values are embedded.
    let text = serde_json::to_string(&document).unwrap();
    assert!(!text.contains(env!("CARGO_MANIFEST_DIR")), "{text}");
    if let Some(home) = std::env::var_os("HOME").filter(|home| home.len() > 1) {
        assert!(!text.contains(home.to_str().unwrap()), "{text}");
    }
}

#[test]
fn relocated_binary_reports_its_own_identity_not_the_current_checkout() {
    let original = json(&due(None, &bin(), &["build-info", "--output", "json"]));

    let install = tempfile::tempdir().unwrap();
    let copied = install.path().join(bin().file_name().unwrap());
    {
        let _guard = COPY_OR_SPAWN.lock().unwrap();
        fs::copy(bin(), &copied).unwrap();
    }

    // A directory that looks like a different checkout, plus Git environment
    // pointing at it. Runtime output must not change.
    let decoy = tempfile::tempdir().unwrap();
    let git_dir = decoy.path().join(".git");
    fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        git_dir.join("refs/heads/main"),
        format!("{}\n", "a".repeat(40)),
    )
    .unwrap();

    let output = capture(
        Command::new(&copied)
            .args(["build-info", "--output", "json"])
            .current_dir(decoy.path())
            .env_clear()
            .env("GIT_DIR", &git_dir)
            .env("GIT_WORK_TREE", decoy.path())
            .stdin(Stdio::null()),
    )
    .unwrap();
    assert_eq!(json(&output), original);
}

#[test]
fn text_summary_and_version_flag() {
    let output = due(None, &bin(), &["build-info"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], format!("due {}", env!("CARGO_PKG_VERSION")));
    assert!(lines[1].starts_with("revision: "), "{text}");
    assert!(lines[2].starts_with("working tree: "), "{text}");
    let features = expected_features();
    let features = if features.is_empty() {
        "none".to_owned()
    } else {
        features.join(", ")
    };
    assert_eq!(lines[3], format!("features: {features}"));
    assert!(text.contains("reproducible-build attestation"), "{text}");

    // --version stays the plain clap version line.
    let version = due(None, &bin(), &["--version"]);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("due {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_and_argument_errors() {
    let help = due(None, &bin(), &["build-info", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("UNKNOWN SOURCE METADATA"), "{help}");
    let overview = String::from_utf8(due(None, &bin(), &["--help"]).stdout).unwrap();
    assert!(
        overview.contains("due build-info --output json"),
        "{overview}"
    );

    let invalid = due(None, &bin(), &["build-info", "--output", "yaml"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
}

// Probe behavior. These use a fixture repository and the test host's `git`.

fn git_available() -> bool {
    capture(
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .is_ok_and(|output| output.status.success())
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = capture(
        Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=Due Test",
                "-c",
                "user.email=due-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null()),
    )
    .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.trim_end().to_owned()
}

const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

fn repository() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    fs::write(root.path().join("lib.rs"), "fn main() {}\n").unwrap();
    fs::write(root.path().join(".gitignore"), "/ignored\n").unwrap();
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-q", "-m", "initial"]);
    root
}

fn git_probe(root: &Path) -> git_identity::Identity {
    probe(root, OsStr::new("git"))
}

#[test]
fn source_archive_without_git_metadata_is_unknown() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("Cargo.toml"), "").unwrap();
    let identity = git_probe(root.path());
    assert_eq!(identity.status, Status::NoGitMetadata);
    assert_eq!(identity.revision, None);
    assert_eq!(identity.tree, Tree::Unknown);
}

#[test]
fn missing_git_executable_is_unknown() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    let missing = root.path().join("no-such-git");
    let identity = probe(root.path(), missing.as_os_str());
    assert_eq!(identity.status, Status::GitUnavailable);
    assert_eq!(identity.revision, None);
    assert_eq!(identity.tree, Tree::Unknown);
}

#[test]
fn archive_unpacked_inside_another_checkout_is_not_attributed_to_it() {
    if !git_available() {
        eprintln!("skipping: git is not available");
        return;
    }
    let outer = repository();
    let package = outer.path().join("unpacked");
    fs::create_dir_all(package.join(".git")).unwrap();
    let identity = git_probe(&package);
    assert_eq!(identity.status, Status::NotRepositoryRoot);
    assert_eq!(identity.revision, None);
    assert_eq!(identity.tree, Tree::Unknown);

    // Without any .git entry Git is never consulted.
    fs::remove_dir(package.join(".git")).unwrap();
    assert_eq!(git_probe(&package).status, Status::NoGitMetadata);
}

#[test]
fn unborn_branch_is_unknown() {
    if !git_available() {
        eprintln!("skipping: git is not available");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    let identity = git_probe(root.path());
    assert_eq!(identity.status, Status::GitError);
    assert_eq!(identity.revision, None);
    assert_eq!(identity.tree, Tree::Unknown);
}

#[test]
fn revision_and_dirty_state_follow_head_index_and_files() {
    if !git_available() {
        eprintln!("skipping: git is not available");
        return;
    }
    let repo = repository();
    let root = repo.path();
    let first = git(root, &["rev-parse", "HEAD"]);
    let identity = git_probe(root);
    assert_eq!(identity.status, Status::Recorded);
    assert_eq!(identity.revision.as_deref(), Some(first.as_str()));
    assert_eq!(identity.tree, Tree::Clean);

    // Ignored files and Cargo's checkout marker are not changes.
    fs::write(root.join("ignored"), "x").unwrap();
    fs::write(root.join(".cargo-ok"), "").unwrap();
    assert_eq!(git_probe(root).tree, Tree::Clean);

    // Untracked, modified and staged sources are.
    fs::write(root.join("new.rs"), "").unwrap();
    assert_eq!(git_probe(root).tree, Tree::Dirty);
    fs::remove_file(root.join("new.rs")).unwrap();
    fs::write(root.join("lib.rs"), "fn main() { }\n").unwrap();
    assert_eq!(git_probe(root).tree, Tree::Dirty);
    git(root, &["add", "lib.rs"]);
    let staged = git_probe(root);
    assert_eq!(staged.tree, Tree::Dirty);
    assert_eq!(staged.revision.as_deref(), Some(first.as_str()));

    git(root, &["commit", "-q", "-m", "second"]);
    let second = git(root, &["rev-parse", "HEAD"]);
    assert_ne!(first, second);
    let committed = git_probe(root);
    assert_eq!(committed.revision.as_deref(), Some(second.as_str()));
    assert_eq!(committed.tree, Tree::Clean);

    git(root, &["checkout", "-q", "--detach", &first]);
    let detached = git_probe(root);
    assert_eq!(detached.revision.as_deref(), Some(first.as_str()));
}

#[test]
fn linked_worktree_reports_its_own_head_and_tree() {
    if !git_available() {
        eprintln!("skipping: git is not available");
        return;
    }
    let repo = repository();
    let parent = tempfile::tempdir().unwrap();
    let tree = parent.path().join("lane");
    let tree_arg = tree.to_str().unwrap();
    let add = ["worktree", "add", "-q", "-b", "lane", tree_arg];
    git(repo.path(), &add);
    fs::write(tree.join("lane.rs"), "").unwrap();
    git(&tree, &["add", "lane.rs"]);
    git(&tree, &["commit", "-q", "-m", "lane"]);

    let identity = git_probe(&tree);
    assert_eq!(identity.status, Status::Recorded);
    let head = git(&tree, &["rev-parse", "HEAD"]);
    assert_eq!(identity.revision.as_deref(), Some(head.as_str()));
    assert_ne!(head, git(repo.path(), &["rev-parse", "HEAD"]));
    assert_eq!(identity.tree, Tree::Clean);

    // A change only in the linked worktree is its own, not the main checkout's.
    fs::write(tree.join("lane.rs"), "// edit\n").unwrap();
    assert_eq!(git_probe(&tree).tree, Tree::Dirty);
    assert_eq!(git_probe(repo.path()).tree, Tree::Clean);
}

// Incremental Cargo builds of a minimal fixture crate that uses the real
// build.rs and probe, with its target directory inside the package.

struct Crate {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Crate {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("fixture");
        let source = Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::copy(source.join("build.rs"), root.join("build.rs")).unwrap();
        fs::copy(
            source.join("build/git_identity.rs"),
            root.join("build/git_identity.rs"),
        )
        .unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "/target\n/Cargo.lock\n").unwrap();
        let fixture = Self { _dir: dir, root };
        fixture.main("");
        fixture
    }

    /// Write src/main.rs printing the recorded identity, plus `extra` code.
    fn main(&self, extra: &str) {
        let source = format!(
            "fn main() {{\n    {extra}\n    println!(\n        \"{{}} {{}} {{}}\",\n        env!(\"DUE_BUILD_SOURCE_STATUS\"),\n        env!(\"DUE_BUILD_TREE\"),\n        env!(\"DUE_BUILD_REVISION\"),\n    );\n}}\n"
        );
        fs::write(self.root.join("src/main.rs"), source).unwrap();
    }

    /// Run one `cargo build` and return whether it succeeded plus the log.
    fn cargo(&self, env: &[(&str, &OsStr)]) -> (bool, String) {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(&self.root)
            .args(["build", "--offline", "-v"])
            .env("CARGO_TARGET_DIR", self.root.join("target"))
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env("CARGO_TERM_COLOR", "never")
            .stdin(Stdio::null());
        for (name, value) in env {
            command.env(name, value);
        }
        let output = capture(&mut command).unwrap();
        let log = String::from_utf8_lossy(&output.stderr).into_owned();
        // The probe runs exactly once per Cargo invocation: never skipped and
        // never retriggered by this build's own outputs.
        let probes = log
            .lines()
            .filter(|line| line.contains("Running `") && line.ends_with("build-script-build`"))
            .count();
        assert_eq!(probes, 1, "{log}");
        (output.status.success(), log)
    }

    /// Build successfully and return what the binary reports.
    fn build_with(&self, env: &[(&str, &OsStr)]) -> String {
        let (success, log) = self.cargo(env);
        assert!(success, "{log}");
        let run = capture(&mut Command::new(self.root.join("target/debug/fixture"))).unwrap();
        assert!(run.status.success(), "{run:?}");
        String::from_utf8(run.stdout).unwrap().trim_end().to_owned()
    }

    fn build(&self) -> String {
        self.build_with(&[])
    }
}

#[test]
fn every_cargo_invocation_records_the_current_source_state() {
    if !git_available() {
        eprintln!("skipping: git is not available");
        return;
    }
    let fixture = Crate::new();
    let root = fixture.root.as_path();

    // Source archive, then metadata appears with no source change.
    assert_eq!(fixture.build(), "no_git_metadata unknown");
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "initial"]);
    let first = git(root, &["rev-parse", "HEAD"]);
    let clean = format!("recorded clean {first}");
    assert_eq!(fixture.build(), clean);
    // An unchanged rebuild refreshes and still reports accurately.
    assert_eq!(fixture.build(), clean);

    // Git environment pointing at another repository is ignored.
    let decoy = repository();
    let decoy_git = decoy.path().join(".git");
    let redirect = [
        ("GIT_DIR", decoy_git.as_os_str()),
        ("GIT_WORK_TREE", decoy.path().as_os_str()),
    ];
    assert_eq!(fixture.build_with(&redirect), clean);

    // A new untracked root file is reported without touching any source.
    fs::write(root.join("notes.txt"), "untracked\n").unwrap();
    assert_eq!(fixture.build(), format!("recorded dirty {first}"));
    fs::remove_file(root.join("notes.txt")).unwrap();
    assert_eq!(fixture.build(), clean);

    // Failed first build: committed source waits for a missing root input.
    fixture.main("let _ = include_str!(\"../new-input.txt\");");
    git(root, &["commit", "-q", "-am", "needs input"]);
    let second = git(root, &["rev-parse", "HEAD"]);
    let (success, log) = fixture.cargo(&[]);
    assert!(!success, "{log}");
    fs::write(root.join("new-input.txt"), "input\n").unwrap();
    assert_eq!(fixture.build(), format!("recorded dirty {second}"));
    git(root, &["add", "new-input.txt"]);
    git(root, &["commit", "-q", "-m", "add input"]);
    let third = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(fixture.build(), format!("recorded clean {third}"));

    // Metadata leaves and returns, again without any source change.
    let parked = fixture.root.parent().unwrap().join("parked-git");
    fs::rename(root.join(".git"), &parked).unwrap();
    assert_eq!(fixture.build(), "no_git_metadata unknown");
    fs::rename(&parked, root.join(".git")).unwrap();
    assert_eq!(fixture.build(), format!("recorded clean {third}"));
}
