//! Command-line entry point and presentation for plugin setup.

use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use clap::{Args, ValueEnum};
use due_plugin::{InstallOptions, Report};

use crate::cli::Output;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Host {
    Both,
    Claude,
    Codex,
}

#[derive(Debug, Args)]
#[command(
    after_help = crate::help::SHORT,
    after_long_help = crate::help::INIT
)]
pub struct InitArgs {
    /// Preview files and native commands without writing or starting a host.
    #[arg(long)]
    pub dry_run: bool,
    /// Hosts to configure; select one if only that host is installed.
    #[arg(long, value_enum, default_value = "both")]
    pub host: Host,
    /// Managed marketplace directory (default: $XDG_DATA_HOME/due or ~/.local/share/due).
    #[arg(long, value_name = "DIR")]
    pub install_dir: Option<PathBuf>,
    /// Claude Code executable, for a nonstandard installation.
    #[arg(long, value_name = "PATH")]
    pub claude_bin: Option<PathBuf>,
    /// Codex executable, for a nonstandard installation.
    #[arg(long, value_name = "PATH")]
    pub codex_bin: Option<PathBuf>,
    /// Human-readable setup report or one JSON document.
    #[arg(long, value_enum, default_value = "text")]
    pub output: Output,
}

pub async fn execute(args: InitArgs) -> ExitCode {
    let cli_version = env!("CARGO_PKG_VERSION");
    let report = match std::env::current_exe() {
        Ok(due_binary) => {
            let options = InstallOptions {
                dry_run: args.dry_run,
                host: match args.host {
                    Host::Both => due_plugin::Host::Both,
                    Host::Claude => due_plugin::Host::Claude,
                    Host::Codex => due_plugin::Host::Codex,
                },
                install_dir: args.install_dir,
                claude_bin: args.claude_bin,
                codex_bin: args.codex_bin,
                due_binary,
                cli_version: cli_version.into(),
            };
            due_plugin::install(&options, crate::shutdown_signal).await
        }
        Err(error) => Report::not_ready(cli_version, Some(error.to_string())),
    };
    let outcome = (|| -> io::Result<()> {
        let mut out = io::stdout().lock();
        if matches!(args.output, Output::Json) {
            serde_json::to_writer(&mut out, &report)?;
            writeln!(out)?;
        } else {
            writeln!(out, "Plugin setup: {}", report.status)?;
            if let Some(plan) = &report.plan {
                writeln!(out, "Marketplace: {}", plan.install_dir.display())?;
                for path in &plan.files {
                    writeln!(out, "  file: {}", path.display())?;
                }
                for step in &plan.commands {
                    writeln!(
                        out,
                        "  {:?} (conditional during apply; argv): {:?} {:?}",
                        step.action, step.executable, step.args
                    )?;
                }
                for issue in &plan.issues {
                    writeln!(out, "  issue: {issue}")?;
                }
            }
            if let Some(error) = &report.error {
                writeln!(out, "{error}")?;
            }
            if let Some(step) = &report.failed_step {
                writeln!(
                    out,
                    "Failed {} step: {:?}\n{}{}",
                    step.command.host, step.error, step.stdout, step.stderr
                )?;
            }
            writeln!(
                out,
                "Matching marketplace registrations are kept; existing Claude user plugins are updated.\nKeep due on PATH. Reload plugins or start a fresh host session after applying setup."
            )?;
        }
        out.flush()
    })();
    if let Err(error) = outcome {
        if error.kind() != io::ErrorKind::BrokenPipe {
            let _ = writeln!(io::stderr(), "due: could not write setup report: {error}");
        }
        return ExitCode::from(1);
    }
    if matches!(report.status, "planned" | "installed") {
        ExitCode::SUCCESS
    } else if report.status == "cancelled" {
        ExitCode::from(130)
    } else {
        ExitCode::from(1)
    }
}
