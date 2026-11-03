//! Records the compile-time source identity reported by `due build-info`.
//!
//! Source archives, missing Git and unrelated parent checkouts all build
//! successfully and record an explicit unknown state.
//!
//! Accuracy over no-change build speed: this script reruns on every Cargo
//! invocation that builds this package, so the reported revision and tree
//! state are never cached from an earlier build. Precise file watches cannot
//! be both cheap and complete: a new untracked file at the package root (for
//! example an input a committed `include_str!` was waiting for) would not
//! rerun them. The cost is that each `cargo build` or `cargo test` recompiles
//! and relinks this package's own targets (the `due` binary and its test
//! binaries); dependency crates, including `due-harness`, stay fresh.

#[path = "build/git_identity.rs"]
mod git_identity;

use std::{env, ffi::OsStr, path::PathBuf};

/// A path inside OUT_DIR that is never created. Cargo treats a missing
/// `rerun-if-changed` path as changed, so the script runs once per Cargo
/// invocation. It is a single named file, not a directory scan, so nothing
/// this build writes can retrigger it within the same invocation.
const ALWAYS_RERUN_SENTINEL: &str = "due-build-info-never-created";

fn main() {
    let root = env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let root = PathBuf::from(root);
    let out_dir = env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR");
    let sentinel = PathBuf::from(out_dir).join(ALWAYS_RERUN_SENTINEL);
    match sentinel.to_str() {
        Some(path) if !path.contains(['\n', '\r']) => {
            println!("cargo::rerun-if-changed={path}");
        }
        _ => {
            // Cargo directives are line-based. A relative ASCII path still
            // works when the absolute output directory is not representable.
            // Select an absent name without creating any source-tree files.
            let mut suffix = 0_u64;
            loop {
                let name = format!("{ALWAYS_RERUN_SENTINEL}-{suffix}");
                if root.join(&name).symlink_metadata().is_err() {
                    println!("cargo::rerun-if-changed={name}");
                    break;
                }
                suffix += 1;
            }
        }
    }

    let identity = git_identity::probe(&root, OsStr::new("git"));
    println!(
        "cargo::rustc-env=DUE_BUILD_SOURCE_STATUS={}",
        identity.status.as_str()
    );
    println!(
        "cargo::rustc-env=DUE_BUILD_REVISION={}",
        identity.revision.as_deref().unwrap_or("")
    );
    println!("cargo::rustc-env=DUE_BUILD_TREE={}", identity.tree.as_str());
}
