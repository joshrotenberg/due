mod claude;
mod codex;

use std::{future::Future, path::PathBuf};

use crate::{
    cli::{Provider, RunArgs},
    result::{ErrorKind, Invocation, RunError, RunResult},
    schema::Schema,
    telemetry::Progress,
};

pub struct Context {
    pub binary: PathBuf,
    pub cwd: PathBuf,
    pub schema: Option<Schema>,
}

impl Context {
    pub fn resolve(run: &RunArgs) -> Result<Self, RunError> {
        let original_cwd =
            std::env::current_dir().map_err(|e| RunError::io("read current directory", e))?;
        let cwd = run
            .cwd
            .as_ref()
            .unwrap_or(&original_cwd)
            .canonicalize()
            .map_err(|e| RunError::invalid(format!("invalid working directory: {e}")))?;
        if !cwd.is_dir() {
            return Err(RunError::invalid("--cwd must name a directory"));
        }
        let requested = run
            .binary
            .clone()
            .unwrap_or_else(|| PathBuf::from(run.provider.as_str()));
        let binary = if requested.is_absolute() {
            requested
        } else if requested.components().count() > 1 {
            original_cwd.join(requested)
        } else {
            match which::which(&requested) {
                Ok(path) => path,
                Err(_) if run.dry_run => requested,
                Err(_) => {
                    return Err(RunError::new(
                        ErrorKind::NotFound,
                        format!("{} was not found on PATH", requested.display()),
                    ));
                }
            }
        };
        let schema = run.schema.as_deref().map(Schema::load).transpose()?;
        Ok(Self {
            binary,
            cwd,
            schema,
        })
    }

    pub fn invocation(&self, args: Vec<String>, prompt: &str) -> Invocation {
        Invocation {
            binary: self.binary.clone(),
            cwd: self.cwd.clone(),
            args,
            stdin_bytes: prompt.len(),
        }
    }
}

pub async fn execute<C>(run: &RunArgs, prompt: &str, progress: &Progress, cancel: C) -> RunResult
where
    C: Future<Output = ()> + Send,
{
    let context = match Context::resolve(run) {
        Ok(context) => context,
        Err(error) => return RunResult::new(run.provider).fail(error),
    };
    let result = match run.provider {
        Provider::Claude => claude::execute(run, &context, prompt, progress, cancel).await,
        Provider::Codex => codex::execute(run, &context, prompt, progress, cancel).await,
    };
    if context.schema.is_some() {
        crate::schema::normalize(result)
    } else {
        result
    }
}
