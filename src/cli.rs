use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "One CLI for Claude Code and Codex",
    after_help = "Start with `due run --help` for execution options.\nInstall the bundled plugin with `due init`; preview with `due init --dry-run`.\nAgents can discover bundled guidance with `due skills`."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Experimental resident sessions over MCP stdio (one session per process).
    #[cfg(feature = "mcp")]
    Mcp,
    /// Run one noninteractive turn, optionally continuing an existing session.
    Run(Box<RunCommand>),
    /// Install the bundled delegate plugin locally; no model work is started.
    Init(crate::init::InitArgs),
    /// Discover and read agent skills embedded in this binary; no provider is started.
    Skills(SkillsArgs),
    /// Export a versioned JSON Schema locally; no provider is started.
    Schema(crate::contracts::SchemaArgs),
    /// Summarize an existing progress trace locally; no provider is started.
    Trace(crate::trace_summary::TraceArgs),
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  due skills\n  due skills list --output json\n  due skills show run\n  due skills show contracts\n  due skills show observe\n\nSkills are local, read-only guidance for this installed version."
)]
pub struct SkillsArgs {
    /// Human-readable guidance or a versioned JSON document.
    #[arg(long, value_enum, default_value = "text", global = true)]
    pub output: Output,

    #[command(subcommand)]
    pub command: Option<SkillsCommand>,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCommand {
    /// List available names and descriptions (also the default for `due skills`).
    List,
    /// Print one complete embedded SKILL.md.
    Show {
        /// Skill name from `due skills list`.
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "mcp", derive(tower_mcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "tower_mcp::schemars"))]
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

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "mcp", derive(tower_mcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "tower_mcp::schemars"))]
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
#[command(
    group(ArgGroup::new("prompt_source").args(["prompt", "stdin"])),
    group(ArgGroup::new("progress_sink").multiple(true).args(["trace", "trace_file"])),
    after_help = "Examples:\n  due run --provider claude -p 'Explain this repository'\n  due run --provider codex --stdin --output json --trace\n  due run --request profile.json --provider codex --stdin --output json\n  due run --request task.json --dry-run --output json\n  due run --request profile.json -p 'Review changes' --no-ephemeral -- --max-turns 5\n  due run --provider codex -p 'Summarize' --schema answer.schema.json --output json\n  due run --request - --output json --trace=json\n\nA request file may be a partial profile; explicit execution flags override its values.\nNative options after -- replace the profile's entire native_args list; use\n--clear-native-args for an empty replacement. --no-ephemeral overrides true.\nRequest cwd/schema paths resolve from the caller's cwd, not the profile directory.\n--request - and --stdin cannot share stdin. One invocation runs one foreground turn.\n\n--dry-run validates the merged request and prepares the invocation without spawning.\nIt does not verify provider authentication, model support, or answer-schema support.\nPrompt contents are omitted; native option values and schemas can appear in the plan.\n\nStdout carries the final answer/result; stderr carries diagnostics or progress.\n--output json wraps the result; --schema requests structured model data.\nExecution status, answer validity, review verdict and workspace effects are separate.\nFailure does not undo edits. Inspect effects before an authorized follow-up.\nNo default timeout. Monitor progress; use --timeout for a deliberate deadline.\nProvider settings and permissions apply. Due does not retry automatically.\nFor agent workflows, read `due skills show run`, `contracts`, or `observe`."
)]
pub struct RunCommand {
    /// Read a versioned JSON request/profile; explicit execution flags override it.
    #[arg(long, value_name = "FILE|-", help_heading = "Request and prompt")]
    pub request: Option<PathBuf>,

    /// Provider CLI to invoke; its existing authentication and settings apply.
    #[arg(
        long,
        value_enum,
        required_unless_present = "request",
        help_heading = "Request and prompt"
    )]
    pub provider: Option<Provider>,

    /// Prompt text (always sent to the provider through stdin).
    #[arg(short = 'p', long, value_name = "TEXT", allow_hyphen_values = true, required_unless_present_any = ["request", "stdin"], help_heading = "Request and prompt")]
    pub prompt: Option<String>,

    /// Read the complete prompt from stdin, replacing any profile prompt.
    #[arg(long, help_heading = "Request and prompt")]
    pub stdin: bool,

    /// Native model name or alias; omitted means inherit, then provider default.
    #[arg(long, value_name = "ID", value_parser = clap::builder::NonEmptyStringValueParser::new(), help_heading = "Execution")]
    pub model: Option<String>,

    /// Reasoning effort (provider/model support varies).
    #[arg(long, value_enum, help_heading = "Execution")]
    pub effort: Option<Effort>,

    /// Child working directory; defaults to the caller's current directory.
    #[arg(short = 'C', long, value_name = "DIR", help_heading = "Execution")]
    pub cwd: Option<PathBuf>,

    /// Provider execution deadline, for example 30s or 5m (no default).
    #[arg(long, value_name = "DURATION", value_parser = parse_timeout, help_heading = "Execution")]
    pub timeout: Option<Duration>,

    /// Final answer text or one versioned JSON result, including runtime errors.
    #[arg(long, value_enum, default_value = "text", help_heading = "Output")]
    pub output: Output,

    /// Request a structured answer using this JSON Schema file.
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    pub schema: Option<PathBuf>,

    /// Continue this provider's native session/thread ID with the prompt.
    #[arg(long, value_name = "ID", conflicts_with = "ephemeral", value_parser = clap::builder::NonEmptyStringValueParser::new(), help_heading = "Execution")]
    pub resume: Option<String>,

    /// Ask the provider not to persist this session.
    #[arg(long, conflicts_with = "no_ephemeral", help_heading = "Execution")]
    pub ephemeral: bool,

    /// Override a profile's ephemeral setting to allow native session persistence.
    #[arg(long, help_heading = "Execution")]
    pub no_ephemeral: bool,

    /// Validate and show the merged invocation without spawning a provider.
    #[arg(long, help_heading = "Planning")]
    pub dry_run: bool,

    /// Override the selected provider's executable.
    #[arg(long, value_name = "PATH", help_heading = "Execution")]
    pub binary: Option<PathBuf>,

    /// Show live progress on stderr; bare --trace selects text.
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "text", require_equals = true, help_heading = "Progress")]
    pub trace: Option<TraceFormat>,

    /// Create a new JSONL progress log (existing files are never overwritten).
    #[arg(long, value_name = "PATH", help_heading = "Progress")]
    pub trace_file: Option<PathBuf>,

    /// Waiting heartbeat interval while tracing; defaults to 5s.
    #[arg(long, value_name = "DURATION", value_parser = parse_timeout, help_heading = "Progress")]
    pub trace_interval: Option<Duration>,

    /// Include supported provider activity detail in progress (may contain content).
    #[arg(long, requires = "progress_sink", help_heading = "Progress")]
    pub trace_detail: bool,

    /// Discard all native_args inherited from a profile.
    #[arg(long, conflicts_with = "native_args", help_heading = "Native options")]
    pub clear_native_args: bool,

    /// Provider options replacing profile native_args, forwarded in order without a shell.
    #[arg(
        last = true,
        value_name = "NATIVE_ARGS",
        allow_hyphen_values = true,
        help_heading = "Native options"
    )]
    pub native_args: Vec<String>,
}

/// Fully resolved execution options, shared by both input forms.
#[derive(Debug)]
pub struct RunArgs {
    pub provider: Provider,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub cwd: Option<PathBuf>,
    pub timeout: Option<Duration>,
    pub output: Output,
    pub schema: Option<PathBuf>,
    pub resume: Option<String>,
    pub ephemeral: bool,
    pub dry_run: bool,
    pub binary: Option<PathBuf>,
    pub trace: Option<TraceFormat>,
    pub trace_file: Option<PathBuf>,
    pub trace_interval: Option<Duration>,
    pub trace_detail: bool,
    pub native_args: Vec<String>,
}

pub(crate) fn parse_timeout(value: &str) -> Result<Duration, String> {
    let duration = humantime::parse_duration(value).map_err(|e| e.to_string())?;
    if duration.is_zero() {
        return Err("duration must be greater than zero".into());
    }
    // Duration can represent values beyond the platform's monotonic clock.
    // Reject those before a heartbeat or native execution deadline is scheduled.
    if Instant::now().checked_add(duration).is_none() {
        return Err("duration exceeds this platform's scheduling range".into());
    }
    Ok(duration)
}
