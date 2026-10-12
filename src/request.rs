//! A request/profile supplies defaults for one invocation; explicit CLI values win.

use std::{io, path::PathBuf};

use clap::ValueEnum;
use serde::{Deserialize, Deserializer};

use crate::{
    cli::{Effort, Provider, RunArgs, RunCommand, parse_timeout},
    result::RunError,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    // Missing is allowed for a partial profile, but explicit null is a bad type.
    // Keep strings until after overlay so replaced semantic values are not used.
    #[serde(default, deserialize_with = "present_string")]
    provider: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    prompt: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    cwd: Option<PathBuf>,
    timeout: Option<String>,
    schema: Option<PathBuf>,
    resume: Option<String>,
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    native_args: Vec<String>,
}

fn present_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn read_request(input: &RunCommand) -> Result<Request, RunError> {
    let Some(path) = &input.request else {
        return Ok(Request::default());
    };
    if path.as_os_str() == "-" && input.stdin {
        return Err(RunError::invalid(
            "--request - and --stdin cannot both read stdin; use a request file or --prompt",
        ));
    }
    // Struct deserialization rejects duplicates, unknown keys, wrong JSON types,
    // and trailing documents before overlay. Paths retain caller cwd semantics.
    let source = if path.as_os_str() == "-" {
        io::read_to_string(io::stdin())
    } else {
        std::fs::read_to_string(path)
    }
    .map_err(|error| RunError::invalid(format!("cannot read --request: {error}")))?;
    let request: Request = serde_json::from_str(&source)
        .map_err(|error| RunError::invalid(format!("invalid --request JSON: {error}")))?;
    if request.schema_version != 1 {
        return Err(RunError::invalid(format!(
            "unsupported request schema_version {}; expected 1",
            request.schema_version
        )));
    }
    Ok(request)
}

pub fn resolve(input: RunCommand) -> Result<RunArgs, RunError> {
    let request = read_request(&input)?;
    resolve_loaded(input, request)
}

#[cfg(any(feature = "workflow-spike", feature = "workflow-lua-spike"))]
pub(crate) fn workflow_profile(value: &serde_json::Value) -> Result<RunArgs, String> {
    use clap::Parser;
    let request: Request = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if request.schema_version != 1 {
        return Err("profile request schema_version must be 1".into());
    }
    let cli = crate::cli::Cli::try_parse_from([
        "due",
        "run",
        "--request",
        "-",
        "--prompt",
        "workflow profile check",
        "--dry-run",
    ])
    .map_err(|e| e.to_string())?;
    let crate::cli::Command::Run(input) = cli.command else {
        unreachable!()
    };
    resolve_loaded(*input, request).map_err(|e| e.message)
}

fn resolve_loaded(input: RunCommand, request: Request) -> Result<RunArgs, RunError> {
    let provider = match input.provider {
        Some(provider) => provider,
        None => Provider::from_str(
            request.provider.as_deref().ok_or_else(|| {
                RunError::invalid(
                    "a provider is required after merging the request and CLI options",
                )
            })?,
            false,
        )
        .map_err(|error| RunError::invalid(format!("invalid request provider: {error}")))?,
    };
    let prompt = if input.stdin {
        None
    } else {
        let prompt = input.prompt.or(request.prompt).ok_or_else(|| {
            RunError::invalid("a prompt is required after merging the request and CLI options; use --prompt or --stdin")
        })?;
        if prompt.trim().is_empty() {
            return Err(RunError::invalid("prompt must not be empty"));
        }
        Some(prompt)
    };
    let model = input.model.or(request.model);
    let resume = input.resume.or(request.resume);
    for (name, value) in [("model", &model), ("resume", &resume)] {
        if value.as_ref().is_some_and(String::is_empty) {
            return Err(RunError::invalid(format!("{name} must not be empty")));
        }
    }
    let ephemeral = input.ephemeral || (!input.no_ephemeral && request.ephemeral);
    if resume.is_some() && ephemeral {
        return Err(RunError::invalid(
            "resume conflicts with ephemeral after merging the request and CLI options; use --no-ephemeral",
        ));
    }
    let effort = match input.effort {
        Some(effort) => Some(effort),
        None => request
            .effort
            .as_deref()
            .map(|value| Effort::from_str(value, false))
            .transpose()
            .map_err(|error| RunError::invalid(format!("invalid request effort: {error}")))?,
    };
    let timeout = match input.timeout {
        Some(timeout) => Some(timeout),
        None => request
            .timeout
            .as_deref()
            .map(parse_timeout)
            .transpose()
            .map_err(|error| RunError::invalid(format!("invalid request timeout: {error}")))?,
    };
    let native_args = if input.clear_native_args {
        Vec::new()
    } else if input.native_args.is_empty() {
        request.native_args
    } else {
        input.native_args
    };
    Ok(RunArgs {
        provider,
        prompt,
        model,
        effort,
        cwd: input.cwd.or(request.cwd),
        timeout,
        output: input.output,
        schema: input.schema.or(request.schema),
        resume,
        ephemeral,
        dry_run: input.dry_run,
        binary: input.binary,
        trace: input.trace,
        trace_file: input.trace_file,
        trace_interval: input.trace_interval,
        trace_detail: input.trace_detail,
        #[cfg(any(feature = "workflow-spike", feature = "workflow-lua-spike"))]
        workflow_trace_limit: input.workflow_trace_limit,
        native_args,
    })
}
