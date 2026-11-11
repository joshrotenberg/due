//! Offline machine-readable contracts shipped with this binary.

use std::{
    io::{self, Write},
    process::ExitCode,
};

use clap::{Args, ValueEnum};

#[derive(Debug, Args)]
#[command(after_help = crate::help::SHORT, after_long_help = crate::help::SCHEMA)]
pub struct SchemaArgs {
    /// Contract to print as JSON; no provider, configuration or stdin is read.
    #[arg(value_enum)]
    pub contract: Contract,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Contract {
    /// JSON task or reusable worker profile.
    Request,
    /// One-shot or resident turn result.
    Result,
    /// One progress JSONL record.
    Trace,
    /// Offline summary or parsing-error envelope.
    TraceSummary,
    #[cfg(feature = "harness")]
    /// JavaScript workflow result and evidence paths.
    ScriptResult,
}

impl Contract {
    fn document(self) -> &'static str {
        match self {
            Self::Request => include_str!("../schemas/run-request.schema.json"),
            Self::Result => include_str!("../schemas/run-result.schema.json"),
            Self::Trace => include_str!("../schemas/trace.schema.json"),
            Self::TraceSummary => include_str!("../schemas/trace-summary.schema.json"),
            #[cfg(feature = "harness")]
            Self::ScriptResult => include_str!("../schemas/script-result.schema.json"),
        }
    }
}

pub fn execute(args: &SchemaArgs) -> ExitCode {
    let result: io::Result<()> = (|| {
        let mut stdout = io::stdout().lock();
        stdout.write_all(args.contract.document().as_bytes())?;
        stdout.flush()
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if error.kind() != io::ErrorKind::BrokenPipe {
                eprintln!("due: cannot write contract: {error}");
            }
            ExitCode::from(1)
        }
    }
}
