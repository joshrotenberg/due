use std::{path::PathBuf, time::Duration};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(version, about = "One CLI for Claude Code and Codex")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run one noninteractive turn, optionally continuing an existing session.
    Run(RunArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Output {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TraceFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("prompt_source").required(true).args(["prompt", "stdin"])))]
pub struct RunArgs {
    /// Provider CLI to invoke; its existing authentication and settings apply.
    #[arg(long, value_enum)]
    pub provider: Provider,

    /// Prompt text (always sent to the provider through stdin).
    #[arg(short = 'p', long, value_name = "TEXT", allow_hyphen_values = true)]
    pub prompt: Option<String>,

    /// Read the complete prompt from stdin.
    #[arg(long)]
    pub stdin: bool,

    /// Native model name or alias; omitted means the provider's default.
    #[arg(long, value_name = "ID", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub model: Option<String>,

    /// Reasoning effort (provider/model support varies).
    #[arg(long, value_enum)]
    pub effort: Option<Effort>,

    /// Child working directory; defaults to the current directory.
    #[arg(short = 'C', long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Provider execution deadline, for example 30s or 5m (no default).
    #[arg(long, value_name = "DURATION", value_parser = parse_timeout)]
    pub timeout: Option<Duration>,

    /// Final answer text or one versioned JSON result, including runtime errors.
    #[arg(long, value_enum, default_value = "text")]
    pub output: Output,

    /// Request a structured answer using this JSON Schema file.
    #[arg(long, value_name = "FILE")]
    pub schema: Option<PathBuf>,

    /// Continue this provider's native session/thread ID with the prompt.
    #[arg(long, value_name = "ID", conflicts_with = "ephemeral", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub resume: Option<String>,

    /// Ask the provider not to persist this session.
    #[arg(long)]
    pub ephemeral: bool,

    /// Show the invocation without spawning; prompt contents are omitted.
    #[arg(long)]
    pub dry_run: bool,

    /// Override the selected provider's executable.
    #[arg(long, value_name = "PATH")]
    pub binary: Option<PathBuf>,

    /// Show live progress on stderr; bare --trace selects text.
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "text", require_equals = true)]
    pub trace: Option<TraceFormat>,

    /// Create a new JSONL progress log (existing files are never overwritten).
    #[arg(long, value_name = "PATH")]
    pub trace_file: Option<PathBuf>,

    /// Waiting heartbeat interval while tracing; defaults to 5s.
    #[arg(long, value_name = "DURATION", value_parser = parse_timeout)]
    pub trace_interval: Option<Duration>,

    /// Extra provider options, forwarded in order without a shell.
    #[arg(last = true, value_name = "NATIVE_ARGS", allow_hyphen_values = true)]
    pub native_args: Vec<String>,
}

fn parse_timeout(value: &str) -> Result<Duration, String> {
    let duration = humantime::parse_duration(value).map_err(|e| e.to_string())?;
    if duration.is_zero() {
        return Err("duration must be greater than zero".into());
    }
    Ok(duration)
}
