mod cli;
mod contracts;
mod providers;
mod request;
mod result;
mod schema;
mod skills;
mod telemetry;
mod trace_summary;
mod validate;

use std::{
    io::{self, Write},
    process::ExitCode,
    time::{Duration, Instant},
};

use clap::Parser;

use cli::{Cli, Command, Output, RunArgs, RunCommand};
use result::{RunError, RunResult};

#[tokio::main]
async fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Run(run) => run_command(*run).await,
        Command::Skills(args) => skills::execute(&args),
        Command::Schema(args) => contracts::execute(&args),
        Command::Trace(args) => trace_summary::execute(&args),
    }
}

async fn run_command(input: RunCommand) -> ExitCode {
    let start = Instant::now();
    let output = input.output;
    let explicit_schema = input.schema.is_some();
    let run = match request::resolve(input) {
        Ok(run) => run,
        Err(error) => {
            let mut result = RunResult::rejected(error);
            if explicit_schema {
                result = result.with_schema_requested(true);
            }
            result.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            return finish_output(output, &result, true);
        }
    };
    let (progress, guard) = match telemetry::start(&run) {
        Ok(tracing) => tracing,
        Err(error) => {
            let mut result = RunResult::new(run.provider)
                .with_schema_requested(run.schema.is_some())
                .fail(error);
            result.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            return finish_output(run.output, &result, true);
        }
    };
    progress.started(&run);
    let mut result = match prepare_prompt(&run) {
        Ok(prompt) => {
            progress.prompt_ready(prompt.len());
            let execution = providers::execute(&run, &prompt, &progress, async {
                // Await the wrappers' cancellation cleanup before reporting exit 130.
                let _ = tokio::signal::ctrl_c().await;
                progress.phase("cancel_requested");
            });
            tokio::pin!(execution);
            let period = run.trace_interval.unwrap_or(Duration::from_secs(5));
            let mut heartbeat =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    result = &mut execution => break result,
                    _ = heartbeat.tick(), if progress.enabled() => progress.heartbeat(),
                }
            }
        }
        Err(error) => RunResult::new(run.provider).fail(error),
    };
    result = result.with_schema_requested(run.schema.is_some());
    result.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    result.run_id = progress.run_id().map(str::to_owned);
    progress.finished(&result);
    if let Some(guard) = guard {
        result.trace = Some(guard.finish());
    }
    finish_output(
        run.output,
        &result,
        run.trace.is_none() || !progress.enabled(),
    )
}

fn finish_output(output: Output, result: &RunResult, diagnostics: bool) -> ExitCode {
    if let Err(error) = render(output, result, diagnostics) {
        if error.kind() != io::ErrorKind::BrokenPipe && diagnostics {
            eprintln!("due: failed to write output: {error}");
        }
        return ExitCode::from(1);
    }
    ExitCode::from(result.exit_code())
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

fn render(output: Output, result: &RunResult, diagnostics: bool) -> io::Result<()> {
    // Native diagnostics can contain prompt or tool content. Stderr tracing
    // deliberately emits only safe progress; JSON stdout retains error details.
    if diagnostics {
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
    }
    let mut stdout = io::stdout().lock();
    match output {
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
