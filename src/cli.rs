use std::{path::PathBuf, time::Duration};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Delegate and supervise coding work with Claude Code or Codex",
    after_help = crate::help::SHORT,
    after_long_help = crate::help::overview()
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run one assignment, optionally continuing a saved native session.
    Run(Box<RunCommand>),
    /// Supervise resident turns, streamed items and independent checks in JavaScript.
    #[cfg(feature = "harness")]
    Script(crate::script::ScriptArgs),
    /// Host a resident session over JSON-RPC stdio for a custom client.
    #[cfg(feature = "rpc")]
    #[command(after_help = crate::help::SHORT, after_long_help = crate::help::RPC)]
    Rpc,
    /// Install or update the bundled delegate plugin in local agent hosts.
    Init(crate::init::InitArgs),
    /// Discover and read the guides embedded in this binary.
    Skills(SkillsArgs),
    /// Export a versioned request, result or trace JSON Schema.
    Schema(crate::contracts::SchemaArgs),
    /// Inspect a saved progress trace offline.
    Trace(crate::trace_summary::TraceArgs),
    /// Report this executable's compile-time version, source revision and features.
    BuildInfo(crate::build_info::BuildInfoArgs),
}

#[derive(Debug, Args)]
#[command(
    after_help = crate::help::SHORT,
    after_long_help = crate::help::SKILLS
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
    #[command(after_help = crate::help::SHORT, after_long_help = crate::help::SKILLS)]
    List,
    /// Print one complete embedded SKILL.md.
    #[command(after_help = crate::help::SHORT, after_long_help = crate::help::SKILLS)]
    Show {
        /// Skill name from `due skills list`.
        #[arg(value_name = "NAME")]
        name: String,
    },
}

pub use due::types::{Effort, Provider, parse_timeout};

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

#[derive(Debug, Args)]
#[command(
    group(ArgGroup::new("prompt_source").args(["prompt", "stdin"])),
    group(ArgGroup::new("progress_sink").multiple(true).args(["trace", "trace_file"])),
    after_help = crate::help::SHORT,
    after_long_help = crate::help::RUN
)]
pub struct RunCommand {
    /// Read a versioned JSON request/profile; explicit execution flags override it.
    #[arg(
        long,
        value_name = "FILE|-",
        help_heading = "Request and prompt",
        long_help = "Read a version-1 JSON task or partial profile from a file, or stdin with -. Explicit execution flags override file values; absent flags inherit. Paths inside the request resolve from the caller directory. See due skills show profiles."
    )]
    pub request: Option<PathBuf>,

    /// Provider CLI to invoke; its existing authentication and settings apply.
    #[arg(
        long,
        value_parser = provider_parser(),
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
    #[arg(long, value_parser = effort_parser(), help_heading = "Execution")]
    pub effort: Option<Effort>,

    /// Child working directory; defaults to the caller's current directory.
    #[arg(short = 'C', long, value_name = "DIR", help_heading = "Execution")]
    pub cwd: Option<PathBuf>,

    /// Provider execution deadline, for example 30s or 5m (no default).
    #[arg(long, value_name = "DURATION", value_parser = parse_timeout, help_heading = "Execution")]
    pub timeout: Option<Duration>,

    /// Final answer text or one versioned JSON result, including runtime errors.
    #[arg(
        long,
        value_enum,
        default_value = "text",
        help_heading = "Output",
        long_help = "Select final answer text or one versioned Due JSON result, including runtime errors. This does not ask the model for JSON; --schema supplies a separate answer schema. JSON callers must inspect status and error, then independently verify task effects."
    )]
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

fn provider_parser() -> impl clap::builder::TypedValueParser<Value = Provider> {
    use clap::builder::TypedValueParser;
    clap::builder::PossibleValuesParser::new(["claude", "codex"])
        .map(|value| Provider::from_str(&value, false).expect("validated provider"))
}
fn effort_parser() -> impl clap::builder::TypedValueParser<Value = Effort> {
    use clap::builder::TypedValueParser;
    clap::builder::PossibleValuesParser::new(["low", "medium", "high"])
        .map(|value| Effort::from_str(&value, false).expect("validated effort"))
}
