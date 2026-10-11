mod cli;
mod providers;
mod result;
mod validate;

use std::{
    io::{self, Write},
    process::ExitCode,
    time::Instant,
};

use clap::Parser;

use cli::{Cli, Command, Output, RunArgs};
use result::{RunError, RunResult};

#[tokio::main]
async fn main() -> ExitCode {
    let Cli {
        command: Command::Run(run),
    } = Cli::parse();
    let start = Instant::now();
    let mut result = match prepare_prompt(&run) {
        Ok(prompt) => {
            providers::execute(&run, &prompt, async {
                // Await the wrappers' cancellation cleanup before reporting exit 130.
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        }
        Err(error) => RunResult::new(run.provider).fail(error),
    };
    result.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let exit_code = result.exit_code();
    if let Err(error) = render(&run, &result) {
        if error.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("due: failed to write output: {error}");
        }
        return ExitCode::from(1);
    }
    ExitCode::from(exit_code)
}

fn prepare_prompt(run: &RunArgs) -> Result<String, RunError> {
    validate::native_args(run)?;
    let prompt = match &run.prompt {
        Some(prompt) => prompt.clone(),
        None => io::read_to_string(io::stdin())
            .map_err(|e| RunError::io("read prompt from stdin", e))?,
    };
    if prompt.trim().is_empty() {
        return Err(RunError::invalid("prompt must not be empty"));
    }
    Ok(prompt)
}

fn render(run: &RunArgs, result: &RunResult) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    if !result.diagnostics.is_empty() {
        write!(stderr, "{}", result.diagnostics)?;
        if !result.diagnostics.ends_with('\n') {
            writeln!(stderr)?;
        }
    }
    if let Some(error) = &result.error
        && error.message.trim() != result.diagnostics.trim()
    {
        writeln!(stderr, "due: {}", error.message)?;
    }
    let mut stdout = io::stdout().lock();
    match run.output {
        Output::Json => {
            serde_json::to_writer(&mut stdout, result)?;
            writeln!(stdout)?;
        }
        Output::Text => {
            if let Some(invocation) = &result.invocation {
                writeln!(stdout, "cwd: {}", invocation.cwd.display())?;
                let args = invocation
                    .args
                    .iter()
                    .map(|arg| shell_quote(arg))
                    .collect::<Vec<_>>()
                    .join(" ");
                writeln!(
                    stdout,
                    "command: {} {args}",
                    shell_quote(&invocation.binary.to_string_lossy())
                )?;
                writeln!(
                    stdout,
                    "stdin: {} bytes (prompt omitted)",
                    invocation.stdin_bytes
                )?;
            } else if result.error.is_none() {
                write!(stdout, "{}", result.text)?;
                if !result.text.ends_with('\n') {
                    writeln!(stdout)?;
                }
            }
        }
    }
    stdout.flush()
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-./:=,".contains(ch))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
