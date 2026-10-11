//! Offline machine-readable contracts shipped with this binary.

use std::{
    io::{self, Write},
    process::ExitCode,
};

use clap::{Args, ValueEnum};

#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// Contract to print as JSON; no provider, configuration or stdin is read.
    #[arg(value_enum)]
    pub contract: Contract,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Contract {
    Request,
    Result,
    Trace,
    TraceSummary,
}

impl Contract {
    fn document(self) -> &'static str {
        match self {
            Self::Request => include_str!("../schemas/run-request.schema.json"),
            Self::Result => include_str!("../schemas/run-result.schema.json"),
            Self::Trace => include_str!("../schemas/trace.schema.json"),
            Self::TraceSummary => include_str!("../schemas/trace-summary.schema.json"),
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
