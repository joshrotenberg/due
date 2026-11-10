//! Host-supplied profile catalogs for the MCP spawn tool.

use std::collections::BTreeMap;

use due_core::{Effort, InteractionMode, Provider, WorkerOptions};
use serde::Deserialize;

use crate::Config;

/// Versioned worker settings supplied by the MCP host.
pub struct ProfileCatalog(Vec<Profile>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema_version: u32,
    profiles: Vec<Profile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    name: String,
    provider: Provider,
    model: Option<String>,
    effort: Option<Effort>,
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    native_args: Vec<String>,
    #[serde(default)]
    interaction_mode: InteractionMode,
}

impl ProfileCatalog {
    /// Maximum JSON catalog size in bytes.
    pub const MAX_BYTES: usize = 1024 * 1024;

    /// Parse a version-1 JSON catalog. Worker options are validated against
    /// the connection configuration before serving any tools.
    pub fn from_json(source: &[u8]) -> Result<Self, String> {
        if source.len() > Self::MAX_BYTES {
            return Err("profile catalog exceeds 1 MiB".into());
        }
        let catalog: Catalog = serde_json::from_slice(source)
            .map_err(|error| format!("invalid profile catalog: {error}"))?;
        if catalog.schema_version != 1 {
            return Err("profile catalog schema_version must be 1".into());
        }
        if catalog.profiles.is_empty() {
            return Err("profile catalog must contain at least one profile".into());
        }
        Ok(Self(catalog.profiles))
    }

    pub(crate) fn resolve(
        self,
        config: &Config,
    ) -> Result<BTreeMap<String, WorkerOptions>, String> {
        let mut workers = BTreeMap::new();
        for profile in self.0 {
            let name = profile.name;
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(
                    "profile names must be 1 to 64 ASCII letters, digits, '-' or '_'".into(),
                );
            }
            if workers.contains_key(&name) {
                return Err(format!("duplicate profile {name:?}"));
            }
            let mut worker = config.worker(profile.provider);
            worker.model = profile.model;
            worker.effort = profile.effort;
            worker.ephemeral = profile.ephemeral;
            worker.native_args = profile.native_args;
            worker.interaction_mode = profile.interaction_mode;
            let worker = worker
                .validate()
                .map_err(|error| format!("invalid profile {name:?}: {}", error.message))?;
            workers.insert(name, worker);
        }
        Ok(workers)
    }
}
