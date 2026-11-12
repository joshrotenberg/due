//! Transport-independent provider identities and requested settings.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Native CLI providing a worker's conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Claude Code through claude-wrapper.
    Claude,
    /// Codex through codex-wrapper.
    Codex,
}

impl Provider {
    /// Native executable's default name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// Parse the shared spelling, optionally ignoring ASCII case.
    pub fn from_str(value: &str, ignore_case: bool) -> Result<Self, String> {
        [Self::Claude, Self::Codex]
            .into_iter()
            .find(|p| {
                p.as_str() == value || (ignore_case && p.as_str().eq_ignore_ascii_case(value))
            })
            .ok_or_else(|| format!("invalid provider {value:?}; expected claude or codex"))
    }
}

/// Shared requested effort; native support depends on provider and model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
}

impl Effort {
    /// Shared native spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Parse the shared spelling, optionally ignoring ASCII case.
    pub fn from_str(value: &str, ignore_case: bool) -> Result<Self, String> {
        [Self::Low, Self::Medium, Self::High]
            .into_iter()
            .find(|p| {
                p.as_str() == value || (ignore_case && p.as_str().eq_ignore_ascii_case(value))
            })
            .ok_or_else(|| format!("invalid effort {value:?}; expected low, medium or high"))
    }
}

/// Parse a positive duration that the current monotonic clock can represent.
pub fn parse_timeout(value: &str) -> Result<Duration, String> {
    let duration = humantime::parse_duration(value).map_err(|e| e.to_string())?;
    validate_timeout(duration)?;
    Ok(duration)
}

/// Largest native session or thread ID accepted for resume or fork.
#[cfg(any(feature = "oneshot", feature = "resident"))]
const MAX_NATIVE_ID: usize = 1024 * 1024;

/// Check a native session or thread ID before it reaches a provider.
///
/// Native CLIs receive the ID as its own argument. Claude declares
/// `--resume [value]` with an optional value and Codex takes the ID as a
/// positional, so an ID that starts with `-` would be parsed as an option.
#[cfg(any(feature = "oneshot", feature = "resident"))]
pub(crate) fn validate_native_id(id: &str) -> Result<(), &'static str> {
    if id.trim().is_empty() {
        return Err("resume ID must not be empty");
    }
    if id.len() > MAX_NATIVE_ID {
        return Err("resume ID exceeds 1 MiB");
    }
    if id.starts_with('-') {
        return Err("resume ID must not start with '-'");
    }
    if id.chars().any(char::is_control) {
        return Err("resume ID must not contain control characters");
    }
    Ok(())
}

pub(crate) fn validate_timeout(duration: Duration) -> Result<(), &'static str> {
    if duration.is_zero() {
        return Err("duration must be greater than zero");
    }
    if Instant::now().checked_add(duration).is_none() {
        return Err("duration exceeds this platform's scheduling range");
    }
    Ok(())
}

#[cfg(all(test, any(feature = "oneshot", feature = "resident")))]
mod tests {
    use super::*;

    #[test]
    fn native_ids_are_bounded_and_cannot_look_like_options() {
        for valid in [
            "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
            "thread name with spaces",
            "fix-\u{e9}t\u{e9}",
            &"a".repeat(MAX_NATIVE_ID),
        ] {
            assert_eq!(validate_native_id(valid), Ok(()), "{:.40}", valid);
        }
        for invalid in [
            "",
            " \t",
            "-c",
            "--dangerously-skip-permissions",
            "native\nid",
            "native\u{7f}id",
            &"a".repeat(MAX_NATIVE_ID + 1),
        ] {
            assert!(validate_native_id(invalid).is_err(), "{:.40}", invalid);
        }
    }
}
