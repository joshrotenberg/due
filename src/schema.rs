//! Shared schema transport and JSON result decoding. Providers enforce their
//! supported JSON Schema constraints; Due does not implement a second validator.

use std::{io::Write, path::Path};

use serde_json::Value;

use crate::{
    cli::Provider,
    result::{RunError, RunResult},
};

pub struct Schema {
    json: String,
    path: String,
    // Keep the same private snapshot alive until the provider has settled.
    _snapshot: tempfile::NamedTempFile,
}

impl Schema {
    pub fn load(path: &Path) -> Result<Self, RunError> {
        // The parent process never changes cwd: relative paths belong to the
        // caller, independently of the provider's --cwd.
        let source = std::fs::read_to_string(path)
            .map_err(|error| RunError::invalid(format!("cannot read --schema file: {error}")))?;
        let value: Value = serde_json::from_str(&source)
            .map_err(|error| RunError::invalid(format!("invalid --schema JSON: {error}")))?;
        if !value.is_object() && !value.is_boolean() {
            return Err(RunError::invalid(
                "--schema must contain a JSON object or boolean",
            ));
        }
        let json = value.to_string();
        let mut snapshot = tempfile::Builder::new()
            .prefix("due-schema-")
            .suffix(".json")
            .tempfile()
            .map_err(|error| RunError::io("create schema snapshot", error))?;
        snapshot
            .write_all(json.as_bytes())
            .and_then(|_| snapshot.flush())
            .map_err(|error| RunError::io("write schema snapshot", error))?;
        let path = snapshot
            .path()
            .to_str()
            .ok_or_else(|| {
                RunError::invalid("schema snapshot requires a UTF-8 temporary directory path")
            })?
            .to_owned();
        Ok(Self {
            json,
            path,
            _snapshot: snapshot,
        })
    }

    pub fn json(&self) -> &str {
        &self.json
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

pub fn normalize(mut result: RunResult) -> RunResult {
    if result.invocation.is_some() {
        return result;
    }
    if result.provider == Some(Provider::Codex) {
        match serde_json::from_str::<Value>(&result.text) {
            Ok(value) => result.structured_output = Some(value),
            Err(error) if result.error.is_none() => {
                return result.fail(RunError::protocol(format!(
                    "Codex did not return valid JSON for --schema: {error}"
                )));
            }
            Err(_) => (), // A schema decoding error must not mask a run failure.
        }
    } else if result.structured_output.is_none() && result.error.is_none() {
        return result.fail(RunError::protocol(
            "Claude did not return structured_output for --schema",
        ));
    }
    result
}
