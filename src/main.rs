mod build_info;
mod cli;
mod contracts;
mod help;
mod init;
mod providers;
mod request;
mod result;
#[cfg(feature = "rpc")]
mod rpc;
mod schema;
#[cfg(feature = "harness")]
mod script;
#[cfg(feature = "rpc")]
mod session;
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
        #[cfg(feature = "rpc")]
        Command::Rpc => rpc::execute().await,
        #[cfg(feature = "harness")]
        Command::Script(args) => script::execute(args).await,
        Command::Run(run) => run_command(*run).await,
        Command::Init(args) => init::execute(args).await,
        Command::Skills(args) => skills::execute(&args),
        Command::Schema(args) => contracts::execute(&args),
        Command::Trace(args) => trace_summary::execute(&args),
        Command::BuildInfo(args) => build_info::execute(&args),
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
                .with_requested(run.model.as_deref(), run.effort)
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
            let period = run.trace_interval.unwrap_or(Duration::from_secs(5));
            // Input may have blocked since duration parsing. Check again at the
            // actual scheduling boundary instead of overflowing Instant::add.
            match tokio::time::Instant::now().checked_add(period) {
                None => RunResult::new(run.provider).fail(RunError::invalid(
                    "trace interval exceeds this platform's scheduling range",
                )),
                Some(heartbeat_start) => match shutdown_signal() {
                    Err(error) => RunResult::new(run.provider)
                        .fail(RunError::io("register shutdown signals", error)),
                    Ok(cancel) => {
                        let execution = providers::execute(&run, &prompt, &progress, async {
                            // Await the wrappers' cancellation cleanup before reporting exit 130.
                            cancel.await;
                            progress.phase("cancel_requested");
                        });
                        tokio::pin!(execution);
                        let mut heartbeat = tokio::time::interval_at(heartbeat_start, period);
                        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {
                                result = &mut execution => break result,
                                _ = heartbeat.tick(), if progress.enabled() => progress.heartbeat(),
                            }
                        }
                    }
                },
            }
        }
        Err(error) => RunResult::new(run.provider).fail(error),
    };
    result = result
        .with_requested(run.model.as_deref(), run.effort)
        .with_schema_requested(run.schema.is_some());
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

#[cfg(unix)]
fn shutdown_signal() -> io::Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};

    // Register before spawning: failure must not leave a running provider
    // without a usable cancellation handler.
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    Ok(async move {
        tokio::select! {
            Some(()) = interrupt.recv() => {},
            Some(()) = terminate.recv() => {},
            Some(()) = hangup.recv() => {},
            // A closed receiver is not a shutdown request.
            else => std::future::pending::<()>().await,
        }
    })
}

#[cfg(windows)]
fn shutdown_signal() -> io::Result<impl std::future::Future<Output = ()>> {
    let mut interrupt = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        if interrupt.recv().await.is_none() {
            std::future::pending::<()>().await;
        }
    })
}

fn finish_output(output: Output, result: &RunResult, diagnostics: bool) -> ExitCode {
    if let Err(error) = render(output, result, diagnostics) {
        if error.kind() != io::ErrorKind::BrokenPipe && diagnostics {
            let _ = writeln!(io::stderr().lock(), "due: failed to write output: {error}");
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
    stdout.flush()?;
    drop(stdout);
    // Deliver the result independently of diagnostics: a closed stderr must
    // neither suppress stdout nor replace the run's exit status.
    if diagnostics {
        let _ = write_diagnostics(result);
    }
    Ok(())
}

fn write_diagnostics(result: &RunResult) -> io::Result<()> {
    // Native diagnostics can contain prompt or tool content. Stderr tracing
    // deliberately emits only safe progress; JSON stdout retains error details.
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
    Ok(())
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
