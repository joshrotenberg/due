//! Operator-configured profiles. Tool callers select an entry by name.

use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use due_core::{Effort, InteractionMode, Provider, WorkerOptions};
use serde::Deserialize;

const MAX_BYTES: u64 = 1024 * 1024;

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

pub struct Workers {
    cwd: PathBuf,
    claude_binary: Option<PathBuf>,
    codex_binary: Option<PathBuf>,
    profiles: BTreeMap<String, WorkerOptions>,
}

impl Workers {
    pub fn load(
        cwd: PathBuf,
        claude_binary: Option<PathBuf>,
        codex_binary: Option<PathBuf>,
        catalog: Option<&Path>,
    ) -> Result<Self, String> {
        let mut workers = Self {
            cwd,
            claude_binary,
            codex_binary,
            profiles: BTreeMap::new(),
        };
        if let Some(path) = catalog {
            workers.load_catalog(path)?;
        }
        Ok(workers)
    }

    fn load_catalog(&mut self, path: &Path) -> Result<(), String> {
        if !path.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("profile catalog must be a regular JSON file".into());
        }
        let mut source = Vec::new();
        File::open(path)
            .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut source))
            .map_err(|error| format!("cannot read profile catalog: {error}"))?;
        if source.len() as u64 > MAX_BYTES {
            return Err("profile catalog exceeds 1 MiB".into());
        }
        let catalog: Catalog = serde_json::from_slice(&source)
            .map_err(|error| format!("invalid profile catalog: {error}"))?;
        if catalog.schema_version != 1 {
            return Err("profile catalog schema_version must be 1".into());
        }
        if catalog.profiles.is_empty() {
            return Err("profile catalog must contain at least one profile".into());
        }
        for profile in catalog.profiles {
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
            if self.profiles.contains_key(&name) {
                return Err(format!("duplicate profile {name:?}"));
            }
            let mut worker = self.defaults(profile.provider);
            worker.model = profile.model;
            worker.effort = profile.effort;
            worker.ephemeral = profile.ephemeral;
            worker.native_args = profile.native_args;
            worker.interaction_mode = profile.interaction_mode;
            let worker = worker
                .validate()
                .map_err(|error| format!("invalid profile {name:?}: {}", error.message))?;
            self.profiles.insert(name, worker);
        }
        Ok(())
    }

    fn defaults(&self, provider: Provider) -> WorkerOptions {
        let mut worker = WorkerOptions::new(provider, &self.cwd);
        worker.binary = match provider {
            Provider::Claude => self.claude_binary.clone(),
            Provider::Codex => self.codex_binary.clone(),
        };
        worker
    }

    pub fn select(
        &self,
        provider: Option<Provider>,
        profile: Option<&str>,
    ) -> Result<WorkerOptions, String> {
        match (provider, profile) {
            (Some(provider), None) if self.profiles.is_empty() => Ok(self.defaults(provider)),
            (Some(_), None) => Err("this server requires a named worker profile".into()),
            (None, Some(profile)) => self
                .profiles
                .get(profile)
                .cloned()
                .ok_or_else(|| "unknown worker profile".into()),
            _ => Err("supply exactly one of provider or profile".into()),
        }
    }
}
