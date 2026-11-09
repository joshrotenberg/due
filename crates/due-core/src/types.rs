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
    if duration.is_zero() {
        return Err("duration must be greater than zero".into());
    }
    if Instant::now().checked_add(duration).is_none() {
        return Err("duration exceeds this platform's scheduling range".into());
    }
    Ok(duration)
}
