mod claude;
mod codex;
mod observer;
mod output;
mod policy;
mod schema;

use crate::provenance;
pub use observer::{ActivityKind, ActivityStatus, RunObserver};

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::{
    result::{ErrorKind, Invocation, RunError, RunResult},
    types::{Effort, Provider, validate_native_id, validate_timeout},
};
use schema::Schema;

type Progress = Arc<dyn RunObserver>;

/// Options for a single native provider invocation.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub cwd: Option<PathBuf>,
    /// Optional provider execution budget. It must be positive and fit the monotonic clock.
    pub timeout: Option<Duration>,
    pub schema: Option<PathBuf>,
    pub resume: Option<String>,
    pub ephemeral: bool,
    pub dry_run: bool,
    pub binary: Option<PathBuf>,
    pub native_args: Vec<String>,
}

impl RunOptions {
    /// Use the provider's defaults in the caller's working directory.
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            model: None,
            effort: None,
            cwd: None,
            timeout: None,
            schema: None,
            resume: None,
            ephemeral: false,
            dry_run: false,
            binary: None,
            native_args: Vec::new(),
        }
    }

    /// Validate native arguments that can conflict with managed execution options.
    pub fn validate_native_args(&self) -> Result<(), RunError> {
        policy::native_args(self)
    }
}

struct Context {
    pub binary: PathBuf,
    pub cwd: PathBuf,
    pub schema: Option<Schema>,
}

impl Context {
    pub fn resolve(run: &RunOptions) -> Result<Self, RunError> {
        if let Some(cwd) = &run.cwd {
            require_utf8_path(cwd, "working directory")?;
        }
        if let Some(binary) = &run.binary {
            require_utf8_path(binary, "executable")?;
        }
        let original_cwd =
            std::env::current_dir().map_err(|e| RunError::io("read current directory", e))?;
        let cwd = run
            .cwd
            .as_ref()
            .unwrap_or(&original_cwd)
            .canonicalize()
            .map_err(|e| RunError::invalid(format!("invalid working directory: {e}")))?;
        require_utf8_path(&cwd, "working directory")?;
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
        require_utf8_path(&binary, "executable")?;
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

fn require_utf8_path(path: &Path, label: &str) -> Result<(), RunError> {
    if path.to_str().is_none() {
        return Err(RunError::invalid(format!(
            "{label} path must contain valid UTF-8; choose a UTF-8 path"
        )));
    }
    Ok(())
}

/// Execute or plan one turn. Use `Arc::new(())` for buffered execution, or
/// supply an observer to receive progress from native streaming output.
pub async fn execute<C>(
    run: &RunOptions,
    prompt: &str,
    progress: Arc<dyn RunObserver>,
    cancel: C,
) -> RunResult
where
    C: Future<Output = ()> + Send,
{
    let result = execute_turn(run, prompt, &progress, cancel).await;
    result
        .with_requested(run.model.as_deref(), run.effort)
        .with_schema_requested(run.schema.is_some())
}

async fn execute_turn<C>(
    run: &RunOptions,
    prompt: &str,
    progress: &Progress,
    cancel: C,
) -> RunResult
where
    C: Future<Output = ()> + Send,
{
    let validation = run.validate_native_args().and_then(|()| {
        if prompt.trim().is_empty() {
            Err(RunError::invalid("prompt must not be empty"))
        } else if let Some(Err(message)) = run.resume.as_deref().map(validate_native_id) {
            Err(RunError::invalid(message))
        } else if run.resume.is_some() && run.ephemeral {
            Err(RunError::invalid("resume conflicts with ephemeral"))
        } else if let Some(timeout) = run.timeout {
            validate_timeout(timeout).map_err(RunError::invalid)
        } else {
            Ok(())
        }
    });
    let context = match validation.and_then(|()| Context::resolve(run)) {
        Ok(context) => context,
        Err(error) => {
            return RunResult::new(run.provider)
                .with_schema_requested(run.schema.is_some())
                .fail(error);
        }
    };
    let result = match run.provider {
        Provider::Claude => claude::execute(run, &context, prompt, progress, cancel).await,
        Provider::Codex => codex::execute(run, &context, prompt, progress, cancel).await,
    };
    if context.schema.is_some() {
        schema::normalize(result)
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::Status;

    #[test]
    fn dry_run_rejects_resume_ids_parsed_as_native_options() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        for provider in [Provider::Claude, Provider::Codex] {
            let mut options = RunOptions::new(provider);
            options.dry_run = true;
            options.binary = Some(PathBuf::from("/bin/echo"));
            for id in ["--dangerously-skip-permissions", "-c", "native\nid", " "] {
                options.resume = Some(id.into());
                let result = runtime.block_on(execute(
                    &options,
                    "Continue",
                    Arc::new(()),
                    std::future::pending(),
                ));
                assert!(
                    matches!(result.status, Status::Failed),
                    "{provider:?}, {id:?}: {:?}",
                    result.status
                );
                assert_eq!(result.error.unwrap().kind, ErrorKind::InvalidInput);
                assert!(result.invocation.is_none());
            }
            options.resume = Some("0199a1b2-thread".into());
            let result = runtime.block_on(execute(
                &options,
                "Continue",
                Arc::new(()),
                std::future::pending(),
            ));
            assert!(matches!(result.status, Status::Planned), "{provider:?}");
        }
    }

    #[test]
    fn dry_run_rejects_invalid_typed_timeouts() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        for provider in [Provider::Claude, Provider::Codex] {
            let mut options = RunOptions::new(provider);
            options.dry_run = true;
            for duration in [Duration::ZERO, Duration::MAX] {
                options.timeout = Some(duration);
                let result = runtime.block_on(execute(
                    &options,
                    "Check the timeout",
                    Arc::new(()),
                    std::future::pending(),
                ));
                assert!(
                    matches!(result.status, Status::Failed),
                    "{provider:?}, {duration:?}: {:?}",
                    result.status
                );
                assert_eq!(result.error.unwrap().kind, ErrorKind::InvalidInput);
                assert!(result.invocation.is_none());
            }
        }
    }
}
