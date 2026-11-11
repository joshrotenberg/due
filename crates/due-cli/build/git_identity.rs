//! Compile-time source identity probe.
//!
//! Shared by `build.rs` and `tests/build_info.rs`; it uses only `std`. The
//! runtime binary never runs this code: it reports the values the build script
//! recorded, so inspecting some checkout at runtime cannot masquerade as the
//! identity of the executable.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// Working tree state observed when the build script ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tree {
    Clean,
    Dirty,
    Unknown,
}

impl Tree {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Dirty => "dirty",
            Self::Unknown => "unknown",
        }
    }
}

/// How the source revision was, or was not, obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The supplied root is a Git work tree root and HEAD resolved to a commit.
    Recorded,
    /// No `.git` entry at the supplied root, for example a source archive.
    NoGitMetadata,
    /// The `git` executable could not be started.
    GitUnavailable,
    /// Git resolved a repository whose root differs from the supplied root, for example
    /// an archive unpacked inside an unrelated checkout.
    NotRepositoryRoot,
    /// Git ran but failed, for example an unborn branch or an untrusted owner.
    GitError,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::NoGitMetadata => "no_git_metadata",
            Self::GitUnavailable => "git_unavailable",
            Self::NotRepositoryRoot => "not_repository_root",
            Self::GitError => "git_error",
        }
    }
}

#[derive(Debug)]
pub struct Identity {
    pub status: Status,
    pub revision: Option<String>,
    pub tree: Tree,
}

impl Identity {
    fn unknown(status: Status) -> Self {
        Self {
            status,
            revision: None,
            tree: Tree::Unknown,
        }
    }
}

enum GitFailure {
    Unavailable,
    Failed,
}

/// Environment that could redirect Git away from the checkout being built.
const REDIRECTING_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
];

const HEAD_ARGS: &[&str] = &["rev-parse", "--verify", "-q", "HEAD^{commit}"];

const STATUS_ARGS: &[&str] = &[
    "status",
    "--porcelain=v1",
    "--untracked-files=normal",
    "--ignore-submodules=none",
];

fn git(program: &OsStr, dir: &Path, args: &[&str]) -> Result<String, GitFailure> {
    let mut command = Command::new(program);
    command
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        // Status must not rewrite the index of the checkout being built.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    for name in REDIRECTING_ENV {
        command.env_remove(name);
    }
    let output = command.output().map_err(|_| GitFailure::Unavailable)?;
    if !output.status.success() {
        return Err(GitFailure::Failed);
    }
    String::from_utf8(output.stdout).map_err(|_| GitFailure::Failed)
}

fn line(text: &str) -> &str {
    text.trim_end_matches(['\n', '\r'])
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Probe the checkout at `root` using the given Git executable.
///
/// Git resolves linked worktrees itself (`.git` may be a file), so a worktree
/// reports its own HEAD and working tree.
pub fn probe(root: &Path, git_program: &OsStr) -> Identity {
    if !root.join(".git").exists() {
        return Identity::unknown(Status::NoGitMetadata);
    }
    let failure = |error: GitFailure| match error {
        GitFailure::Unavailable => Status::GitUnavailable,
        GitFailure::Failed => Status::GitError,
    };
    let top = match git(git_program, root, &["rev-parse", "--show-toplevel"]) {
        Ok(top) => top,
        Err(error) => return Identity::unknown(failure(error)),
    };
    if !same_dir(Path::new(line(&top)), root) {
        return Identity::unknown(Status::NotRepositoryRoot);
    }
    let revision = match git(git_program, root, HEAD_ARGS) {
        Ok(text) if is_object_id(line(&text)) => line(&text).to_ascii_lowercase(),
        Ok(_) => return Identity::unknown(Status::GitError),
        Err(error) => return Identity::unknown(failure(error)),
    };
    // Untracked, non-ignored files count: they can be compiled into the binary.
    // Cargo's own Git checkouts (`cargo install --git`) add an untracked
    // `.cargo-ok` marker that is not source and must not count as a change.
    let tree = match git(git_program, root, STATUS_ARGS) {
        Ok(text) if text.lines().all(|entry| entry == "?? .cargo-ok") => Tree::Clean,
        Ok(_) => Tree::Dirty,
        Err(_) => Tree::Unknown,
    };
    Identity {
        status: Status::Recorded,
        revision: Some(revision),
        tree,
    }
}
