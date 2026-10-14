use super::schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub entry: String,
    pub modules: BTreeMap<String, PathBuf>,
    pub due_binary: Option<PathBuf>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub roles: BTreeMap<String, Role>,
    #[serde(default)]
    pub workspaces: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub max_jobs: usize,
    #[serde(default = "one")]
    pub max_active: usize,
}
fn one() -> usize {
    1
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub preferences: Vec<String>,
    #[serde(default)]
    pub native_args: Vec<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub module: String,
    pub profiles: Vec<String>,
    pub workspaces: Vec<String>,
    pub access: Access,
}
#[derive(Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub intent: String,
    #[serde(default)]
    pub skills: Vec<String>,
    pub model: Option<Selection>,
    pub input_schema: Value,
    pub output_schema: Value,
    pub answer_schema: Option<Value>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Selection {
    Exact { exact: Exact },
    Preference { preference: String },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exact {
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}
impl Metadata {
    pub fn validate(&self) -> Result<(), String> {
        if self.intent.trim().is_empty() {
            return Err("metadata intent is empty".into());
        }
        schema::validator(&self.input_schema)?;
        schema::validator(&self.output_schema)?;
        if let Some(schema) = &self.answer_schema {
            schema::validator(schema)?;
        }
        Ok(())
    }
}
impl Config {
    pub(super) fn canonical(mut self) -> Result<Self, String> {
        self.validate()?;
        for p in self
            .modules
            .values_mut()
            .chain(self.workspaces.values_mut())
            .chain(self.due_binary.iter_mut())
        {
            *p = p
                .canonicalize()
                .map_err(|e| format!("configured path {}: {e}", p.display()))?;
        }
        Ok(self)
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("config {}: {e}", path.display()))?;
        let mut config: Self =
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("config {}: {e}", path.display()))?;
        let base = path.parent().unwrap();
        for p in config
            .modules
            .values_mut()
            .chain(config.workspaces.values_mut())
            .chain(config.due_binary.iter_mut())
        {
            *p = base.join(&*p);
        }
        config.canonical()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.modules.len() > 64 || !self.modules.contains_key(&self.entry) {
            return Err("entry must name one of at most 64 configured modules".into());
        }
        if self.max_jobs > 128 || !(1..=32).contains(&self.max_active) {
            return Err("max_jobs must be 0..128 and max_active 1..32".into());
        }
        for path in self
            .modules
            .values()
            .chain(self.workspaces.values())
            .chain(self.due_binary.iter())
        {
            if !path.is_absolute() {
                return Err("library config paths must be absolute".into());
            }
        }
        for (name, profile) in &self.profiles {
            if !matches!(profile.provider.as_str(), "claude" | "codex") {
                return Err(format!("profile {name}: provider must be claude or codex"));
            }
            if profile.model.as_ref().is_some_and(|m| m.is_empty())
                || profile
                    .effort
                    .as_deref()
                    .is_some_and(|e| !matches!(e, "low" | "medium" | "high"))
            {
                return Err(format!(
                    "profile {name}: invalid model/effort; frozen Due accepts low, medium, high"
                ));
            }
            if profile.native_args.iter().any(|a| {
                [
                    "--cd",
                    "-C",
                    "--cwd",
                    "--json-schema",
                    "--output-schema",
                    "--model",
                    "-m",
                ]
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
            }) {
                return Err(format!(
                    "profile {name}: native path/model/schema override is not a permission flag"
                ));
            }
        }
        for (name, role) in &self.roles {
            if !self.modules.contains_key(&role.module)
                || role.profiles.iter().any(|p| !self.profiles.contains_key(p))
                || role
                    .workspaces
                    .iter()
                    .any(|p| !self.workspaces.contains_key(p))
            {
                return Err(format!(
                    "role {name}: unknown module/profile/workspace grant"
                ));
            }
        }
        Ok(())
    }
    pub fn select(&self, role: &Role, meta: &Metadata) -> Result<(String, Profile), String> {
        role.profiles
            .iter()
            .find_map(|name| {
                let p = self.profiles.get(name)?;
                let matches = match &meta.model {
                    Some(Selection::Exact { exact }) => {
                        p.provider == exact.provider
                            && p.model.as_ref() == Some(&exact.model)
                            && p.effort == exact.effort
                    }
                    Some(Selection::Preference { preference }) => {
                        p.preferences.contains(preference)
                    }
                    None => true,
                };
                matches.then(|| (name.clone(), p.clone()))
            })
            .ok_or_else(|| "no caller-granted profile matches module model selection".into())
    }
}
