//! A request is an alternate representation of one invocation, not a config layer.

use std::{io, path::PathBuf};

use serde::Deserialize;

use crate::{
    cli::{Effort, Provider, RunArgs, RunCommand, parse_timeout},
    result::RunError,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    provider: Provider,
    prompt: String,
    model: Option<String>,
    effort: Option<Effort>,
    cwd: Option<PathBuf>,
    timeout: Option<String>,
    schema: Option<PathBuf>,
    resume: Option<String>,
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    native_args: Vec<String>,
}

pub fn resolve(mut input: RunCommand) -> Result<RunArgs, RunError> {
    if let Some(path) = &input.request {
        // Read through EOF and deserialize the struct directly: this rejects
        // duplicate keys, unknown fields, and extra documents without a lossy
        // intermediate Value. Paths keep the caller's normal cwd semantics.
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
        if request.prompt.trim().is_empty() {
            return Err(RunError::invalid("request prompt must not be empty"));
        }
        for (name, value) in [("model", &request.model), ("resume", &request.resume)] {
            if value.as_ref().is_some_and(String::is_empty) {
                return Err(RunError::invalid(format!(
                    "request {name} must not be empty"
                )));
            }
        }
        if request.resume.is_some() && request.ephemeral {
            return Err(RunError::invalid("request resume conflicts with ephemeral"));
        }
        input.provider = Some(request.provider);
        input.prompt = Some(request.prompt);
        input.model = request.model;
        input.effort = request.effort;
        input.cwd = request.cwd;
        input.timeout = request
            .timeout
            .as_deref()
            .map(parse_timeout)
            .transpose()
            .map_err(|error| RunError::invalid(format!("invalid request timeout: {error}")))?;
        input.schema = request.schema;
        input.resume = request.resume;
        input.ephemeral = request.ephemeral;
        input.native_args = request.native_args;
    }
    Ok(RunArgs {
        provider: input
            .provider
            .ok_or_else(|| RunError::invalid("a provider is required"))?,
        prompt: input.prompt,
        model: input.model,
        effort: input.effort,
        cwd: input.cwd,
        timeout: input.timeout,
        output: input.output,
        schema: input.schema,
        resume: input.resume,
        ephemeral: input.ephemeral,
        dry_run: input.dry_run,
        binary: input.binary,
        trace: input.trace,
        trace_file: input.trace_file,
        trace_interval: input.trace_interval,
        native_args: input.native_args,
    })
}
