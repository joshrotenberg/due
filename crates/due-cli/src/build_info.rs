//! `due build-info`: the compile-time identity of this executable.
//!
//! Every value is fixed when the binary is built (see `build.rs`). Nothing is
//! read from the filesystem, environment or a Git checkout at runtime. This is
//! a descriptive snapshot, not a reproducible-build attestation.

use std::{
    io::{self, Write},
    process::ExitCode,
};

use clap::Args;
use serde::Serialize;

use crate::cli::Output;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Args)]
#[command(
    after_help = crate::help::SHORT,
    after_long_help = crate::help::BUILD_INFO
)]
pub struct BuildInfoArgs {
    /// Human-readable summary or a versioned JSON document.
    #[arg(long, value_enum, default_value = "text")]
    pub output: Output,
}

/// Due feature flags that change the commands and contracts of a build.
const FEATURES: &[(&str, bool)] = &[
    ("core", cfg!(feature = "core")),
    ("harness", cfg!(feature = "harness")),
    ("mcp-stdio", cfg!(feature = "mcp-stdio")),
    ("rpc", cfg!(feature = "rpc")),
];

#[derive(Debug, Serialize)]
struct BuildInfo {
    schema_version: u32,
    package: Package,
    source: Source,
    features: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct Package {
    name: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct Source {
    /// How the revision was obtained, or why it is unknown.
    status: &'static str,
    /// Full commit ID of HEAD when built; null when unknown.
    revision: Option<&'static str>,
    /// clean, dirty or unknown.
    tree: &'static str,
}

fn known(value: &'static str, allowed: &[&'static str]) -> &'static str {
    if allowed.contains(&value) {
        value
    } else {
        "unknown"
    }
}

fn current() -> BuildInfo {
    let status = known(
        env!("DUE_BUILD_SOURCE_STATUS"),
        &[
            "recorded",
            "no_git_metadata",
            "git_unavailable",
            "not_repository_root",
            "git_error",
        ],
    );
    let revision = match env!("DUE_BUILD_REVISION") {
        "" => None,
        revision => Some(revision),
    };
    BuildInfo {
        schema_version: SCHEMA_VERSION,
        package: Package {
            name: env!("CARGO_BIN_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        },
        source: Source {
            status,
            revision,
            tree: known(env!("DUE_BUILD_TREE"), &["clean", "dirty"]),
        },
        features: FEATURES
            .iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(name, _)| *name)
            .collect(),
    }
}

fn write_text(out: &mut impl Write, info: &BuildInfo) -> io::Result<()> {
    writeln!(out, "{} {}", info.package.name, info.package.version)?;
    match info.source.revision {
        Some(revision) => writeln!(out, "revision: {revision}")?,
        None => writeln!(out, "revision: unknown ({})", info.source.status)?,
    }
    writeln!(out, "working tree: {}", info.source.tree)?;
    if info.features.is_empty() {
        writeln!(out, "features: none")?;
    } else {
        writeln!(out, "features: {}", info.features.join(", "))?;
    }
    writeln!(
        out,
        "Recorded when this binary was built; not a reproducible-build attestation."
    )
}

pub fn execute(args: &BuildInfoArgs) -> ExitCode {
    let info = current();
    let mut stdout = io::stdout().lock();
    let outcome = match args.output {
        Output::Json => serde_json::to_writer(&mut stdout, &info)
            .map_err(io::Error::from)
            .and_then(|_| writeln!(stdout)),
        Output::Text => write_text(&mut stdout, &info),
    }
    .and_then(|_| stdout.flush());
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if error.kind() != io::ErrorKind::BrokenPipe {
                let _ = writeln!(
                    io::stderr().lock(),
                    "due: failed to write build info: {error}"
                );
            }
            ExitCode::from(1)
        }
    }
}
