use std::{io, path::PathBuf};

use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::types::{Effort, Provider};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Planned,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub schema_version: u32,
    pub run_id: Option<String>,
    pub trace: Option<TraceSummary>,
    pub permission_denials: PermissionDenials,
    /// Null only when a JSON request was rejected before resolving an invocation.
    pub provider: Option<Provider>,
    pub provenance: Provenance,
    pub status: Status,
    pub text: String,
    /// Native structured data, or a decoded final answer requested with --schema.
    pub structured_output: Option<serde_json::Value>,
    /// Due does not independently validate answers against the requested schema.
    pub answer_schema_validation: AnswerSchemaValidation,
    pub session_id: Option<String>,
    pub usage: Option<Usage>,
    pub cost_usd: Option<f64>,
    pub duration_ms: u64,
    pub error: Option<RunError>,
    pub invocation: Option<Invocation>,
    pub diagnostics: String,
}

/// This describes Due's independent validation, not a provider's enforcement.
#[derive(Debug, Default, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerSchemaValidation {
    /// Intake failed before common schema intent could be resolved.
    #[serde(rename = "not_resolved")]
    Unresolved,
    #[default]
    NotRequested,
    NotPerformed,
}

/// Resolved caller intent is independent of evidence from native turn metadata.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Provenance {
    /// None means intake failed before the merged request was resolved.
    pub requested: Option<RequestedSettings>,
    pub reported: ReportedSettings,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct RequestedSettings {
    pub model: Option<String>,
    pub effort: Option<&'static str>,
}

impl RequestedSettings {
    pub fn new(model: Option<&str>, effort: Option<Effort>) -> Self {
        Self {
            model: model.map(str::to_owned),
            effort: effort.map(Effort::as_str),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReportedSettings {
    /// Unique native model IDs, in observation order, never requested aliases.
    pub models: Vec<String>,
    /// Neither pinned native turn surface currently reports actual effort.
    pub effort: Option<String>,
    /// At least one model ID exceeded the count or byte bound.
    pub truncated: bool,
}

impl ReportedSettings {
    pub fn model(&mut self, model: &str) {
        if model.trim().is_empty() || model.chars().any(char::is_control) {
            return;
        }
        if self.models.iter().any(|known| known == model) {
            return;
        }
        // Do not truncate an ID into a different, apparently valid model name.
        if model.len() > 256 || self.models.len() == 16 {
            self.truncated = true;
            return;
        }
        self.models.push(model.to_owned());
    }

    pub fn merge(&mut self, other: Self) {
        for model in other.models {
            self.model(&model);
        }
        self.truncated |= other.truncated;
    }
}

/// Bounded metadata from Claude's native terminal records.
#[derive(Debug, Default, Clone, Serialize)]
pub struct PermissionDenials {
    pub items: Vec<PermissionDenial>,
    pub incomplete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionDenial {
    pub tool_name: String,
    pub tool_use_id: String,
}

impl Serialize for RunResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let has_denials =
            !self.permission_denials.items.is_empty() || self.permission_denials.incomplete;
        let count = 13
            + usize::from(has_denials)
            + usize::from(self.run_id.is_some())
            + usize::from(self.trace.is_some())
            + usize::from(self.invocation.is_some());
        let mut fields = serializer.serialize_struct("RunResult", count)?;
        fields.serialize_field("schema_version", &self.schema_version)?;
        if let Some(run_id) = &self.run_id {
            fields.serialize_field("run_id", run_id)?;
        }
        if let Some(trace) = &self.trace {
            fields.serialize_field("trace", trace)?;
        }
        if has_denials {
            fields.serialize_field("permission_denials", &self.permission_denials)?;
        }
        fields.serialize_field("provider", &self.provider)?;
        fields.serialize_field("provenance", &self.provenance)?;
        fields.serialize_field("status", &self.status)?;
        fields.serialize_field("text", &self.text)?;
        fields.serialize_field("structured_output", &self.structured_output)?;
        // Compute presence at serialization time so Some(Null) cannot be confused
        // with None, and provider adapters cannot leave a separate flag stale.
        fields.serialize_field(
            "structured_output_present",
            &self.structured_output.is_some(),
        )?;
        fields.serialize_field("answer_schema_validation", &self.answer_schema_validation)?;
        fields.serialize_field("session_id", &self.session_id)?;
        fields.serialize_field("usage", &self.usage)?;
        fields.serialize_field("cost_usd", &self.cost_usd)?;
        fields.serialize_field("duration_ms", &self.duration_ms)?;
        fields.serialize_field("error", &self.error)?;
        if let Some(invocation) = &self.invocation {
            fields.serialize_field("invocation", invocation)?;
        }
        fields.end()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceSummary {
    pub incomplete: bool,
    pub dropped_records: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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
        Self::empty(Some(provider))
    }

    pub fn rejected(error: RunError) -> Self {
        Self {
            answer_schema_validation: AnswerSchemaValidation::Unresolved,
            ..Self::empty(None)
        }
        .fail(error)
    }

    fn empty(provider: Option<Provider>) -> Self {
        Self {
            schema_version: 1,
            run_id: None,
            trace: None,
            permission_denials: PermissionDenials::default(),
            provider,
            provenance: Provenance {
                requested: provider.map(|_| RequestedSettings::default()),
                ..Provenance::default()
            },
            status: Status::Succeeded,
            text: String::new(),
            structured_output: None,
            answer_schema_validation: AnswerSchemaValidation::NotRequested,
            session_id: None,
            usage: None,
            cost_usd: None,
            duration_ms: 0,
            error: None,
            invocation: None,
            diagnostics: String::new(),
        }
    }

    pub fn with_schema_requested(mut self, requested: bool) -> Self {
        self.answer_schema_validation = if requested {
            AnswerSchemaValidation::NotPerformed
        } else {
            AnswerSchemaValidation::NotRequested
        };
        self
    }

    pub fn with_requested(mut self, model: Option<&str>, effort: Option<Effort>) -> Self {
        self.provenance.requested = Some(RequestedSettings::new(model, effort));
        self
    }

    pub fn fail(mut self, error: RunError) -> Self {
        let mut error = error;
        error.message = crate::diagnostics::diagnostic(&error.message);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn output_presence_is_computed_without_losing_explicit_null() {
        let mut result = RunResult::new(Provider::Claude);
        for (output, present) in [
            (None, false),
            (Some(Value::Null), true),
            (Some(json!({})), true),
            (None, false),
        ] {
            result.structured_output = output.clone();
            let encoded = serde_json::to_value(&result).unwrap();
            assert_eq!(encoded["structured_output_present"], present);
            assert_eq!(encoded["structured_output"], output.unwrap_or(Value::Null));
        }
    }
}
