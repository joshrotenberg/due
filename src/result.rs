use std::{io, path::PathBuf};

use serde::Serialize;

use crate::cli::Provider;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Planned,
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<TraceSummary>,
    pub provider: Provider,
    pub status: Status,
    pub text: String,
    /// Native structured data, or a decoded final answer requested with --schema.
    pub structured_output: Option<serde_json::Value>,
    pub session_id: Option<String>,
    pub usage: Option<Usage>,
    pub cost_usd: Option<f64>,
    pub duration_ms: u64,
    pub error: Option<RunError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation: Option<Invocation>,
    #[serde(skip)]
    pub diagnostics: String,
}

#[derive(Debug, Serialize)]
pub struct TraceSummary {
    pub incomplete: bool,
    pub dropped_records: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Usage {
    /// Native input counter; cache inclusion differs between providers.
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    /// Only the provider's explicit total; do not guess overlapping bucket totals.
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Invocation {
    pub binary: PathBuf,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub stdin_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidInput,
    Authentication,
    MaxTurns,
    MaxBudget,
    Configuration,
    SessionNotFound,
    Unsupported,
    Provider,
    Protocol,
    Io,
    Timeout,
    Cancelled,
    NotFound,
}

#[derive(Debug, Serialize)]
pub struct RunError {
    pub kind: ErrorKind,
    pub message: String,
    pub provider_exit_code: Option<i32>,
}

impl RunError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            provider_exit_code: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Protocol, message)
    }

    pub fn io(context: &str, error: io::Error) -> Self {
        Self::new(ErrorKind::Io, format!("{context}: {error}"))
    }

    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.provider_exit_code = Some(code);
        self
    }

    pub fn exit_code(&self) -> u8 {
        match self.kind {
            ErrorKind::InvalidInput => 2,
            ErrorKind::Timeout => 124,
            ErrorKind::Cancelled => 130,
            ErrorKind::NotFound => 127,
            _ => 1,
        }
    }
}

impl RunResult {
    pub fn new(provider: Provider) -> Self {
        Self {
            schema_version: 1,
            run_id: None,
            trace: None,
            provider,
            status: Status::Succeeded,
            text: String::new(),
            structured_output: None,
            session_id: None,
            usage: None,
            cost_usd: None,
            duration_ms: 0,
            error: None,
            invocation: None,
            diagnostics: String::new(),
        }
    }

    pub fn fail(mut self, error: RunError) -> Self {
        self.status = match error.kind {
            ErrorKind::Timeout => Status::TimedOut,
            ErrorKind::Cancelled => Status::Cancelled,
            _ => Status::Failed,
        };
        self.error = Some(error);
        self
    }

    pub fn planned(provider: Provider, invocation: Invocation) -> Self {
        Self {
            status: Status::Planned,
            invocation: Some(invocation),
            ..Self::new(provider)
        }
    }

    pub fn exit_code(&self) -> u8 {
        self.error.as_ref().map_or(0, RunError::exit_code)
    }
}
