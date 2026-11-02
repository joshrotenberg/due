//! Resolve existing JSON profiles at the resident-session boundary.

use std::{fs::OpenOptions as FileOptions, io::Read, path::PathBuf};

use clap::ValueEnum;
use serde::{Deserialize, Deserializer};

use super::OpenOptions;
use crate::{
    cli::{Effort, Provider},
    request::Request,
};

const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

/// Unlike `Option<T>`, a nullable override distinguishes absence from JSON null.
#[derive(Debug, Default)]
enum Override<T> {
    #[default]
    Inherit,
    Set(Option<T>),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Override<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(Self::Set)
    }
}

impl<T> Override<T> {
    fn or(self, inherited: Option<T>) -> Option<T> {
        match self {
            Self::Inherit => inherited,
            Self::Set(value) => value,
        }
    }
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Flat RPC parameters overlay a version-1 request/profile, if supplied.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenInput {
    #[serde(default, deserialize_with = "present")]
    request: Option<PathBuf>,
    #[serde(default, deserialize_with = "present")]
    provider: Option<String>,
    #[serde(default)]
    model: Override<String>,
    #[serde(default)]
    effort: Override<String>,
    #[serde(default)]
    cwd: Override<PathBuf>,
    binary: Option<PathBuf>,
    #[serde(default)]
    resume: Override<String>,
    #[serde(default, deserialize_with = "present")]
    ephemeral: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    stream: Option<bool>,
    #[serde(default)]
    timeout: Override<String>,
    #[serde(default, deserialize_with = "present")]
    native_args: Option<Vec<String>>,
}

impl OpenInput {
    pub(crate) async fn resolve(self) -> Result<OpenOptions, String> {
        let profile = match self.request {
            Some(path) => tokio::task::spawn_blocking(move || read_profile(path))
                .await
                .map_err(|error| format!("profile reader failed: {error}"))??,
            None => Request::default(),
        };
        if profile.prompt.is_some() {
            return Err("request profile contains prompt; supply prompts with turn.start".into());
        }
        if profile.schema.is_some() {
            return Err(
                "request profile contains schema; resident answer schemas are unsupported".into(),
            );
        }
        let provider = self
            .provider
            .or(profile.provider)
            .ok_or("provider is required after merging request and session.open options")?;
        let provider = Provider::from_str(&provider, false)
            .map_err(|error| format!("invalid provider: {error}"))?;
        let effort = self
            .effort
            .or(profile.effort)
            .map(|value| Effort::from_str(&value, false))
            .transpose()
            .map_err(|error| format!("invalid effort: {error}"))?;
        Ok(OpenOptions {
            provider,
            model: self.model.or(profile.model),
            effort,
            cwd: self.cwd.or(profile.cwd),
            binary: self.binary,
            resume: self.resume.or(profile.resume),
            ephemeral: self.ephemeral.unwrap_or(profile.ephemeral),
            stream: self.stream.unwrap_or(false),
            timeout: self.timeout.or(profile.timeout),
            native_args: self.native_args.unwrap_or(profile.native_args),
        })
    }
}

fn read_profile(path: PathBuf) -> Result<Request, String> {
    if path.as_os_str() == "-" {
        return Err("request cannot be '-': stdin carries RPC frames; use a JSON file".into());
    }
    let mut options = FileOptions::new();
    options.read(true);
    // Opening a FIFO must not strand a blocking reader and prevent RPC shutdown.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|error| format!("cannot read request profile: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect request profile: {error}"))?;
    if !metadata.is_file() {
        return Err("request profile must be a regular file".into());
    }
    let mut source = String::new();
    file.take(MAX_PROFILE_BYTES + 1)
        .read_to_string(&mut source)
        .map_err(|error| format!("cannot read request profile: {error}"))?;
    if source.len() as u64 > MAX_PROFILE_BYTES {
        return Err("request profile exceeds 1 MiB".into());
    }
    crate::request::decode(&source).map_err(|error| error.message)
}

/// Resident transports accept only these audited policy options. The one-shot
/// CLI escape hatch remains independent because its argv contract is different.
pub(super) fn validate_native_args(provider: Provider, arguments: &[String]) -> Result<(), String> {
    let mut index = 0;
    while index < arguments.len() {
        let token = &arguments[index];
        let (name, attached) =
            if provider == Provider::Codex && token.starts_with("-c") && token != "-c" {
                (
                    "-c",
                    Some(token[2..].strip_prefix('=').unwrap_or(&token[2..])),
                )
            } else {
                let (name, value) = token
                    .split_once('=')
                    .map_or((token.as_str(), None), |(n, v)| (n, Some(v)));
                (name, value)
            };
        let recognized = match provider {
            Provider::Codex => matches!(name, "-c" | "--config"),
            Provider::Claude => matches!(
                name,
                "--allowedTools"
                    | "--allowed-tools"
                    | "--disallowedTools"
                    | "--disallowed-tools"
                    | "--permission-mode"
            ),
        };
        if !recognized {
            return Err(format!(
                "native option {name:?} is unsupported by resident {} sessions",
                provider.as_str()
            ));
        }
        let value = match attached {
            Some(value) => value,
            None => {
                index += 1;
                arguments
                    .get(index)
                    .map(String::as_str)
                    .ok_or_else(|| format!("native option {name} needs a value"))?
            }
        };
        if value.trim().is_empty() || value.starts_with('-') {
            return Err(format!("native option {name} needs a nonempty value"));
        }
        match provider {
            Provider::Codex => validate_codex_config(value)?,
            Provider::Claude if name == "--permission-mode" => {
                if !matches!(
                    value,
                    "default" | "acceptEdits" | "plan" | "dontAsk" | "bypassPermissions"
                ) {
                    return Err(format!(
                        "unsupported resident Claude permission mode {value:?}"
                    ));
                }
            }
            Provider::Claude => (),
        }
        index += 1;
    }
    Ok(())
}

fn validate_codex_config(source: &str) -> Result<(), String> {
    let key = source
        .split_once('=')
        .map(|(key, _)| key.trim())
        .ok_or("resident Codex config must be a key=value assignment")?;
    let table = source
        .parse::<toml::Table>()
        .map_err(|error| format!("resident Codex config must be one TOML key/value: {error}"))?;
    let mut value = toml::Value::Table(table);
    let mut path = Vec::new();
    while let toml::Value::Table(table) = value {
        if table.len() != 1 {
            return Err("resident Codex config must contain exactly one supported key".into());
        }
        let (key, next) = table.into_iter().next().unwrap();
        path.push(key);
        value = next;
    }
    // The native CLI consumes key=value, not a whole TOML document. Do not
    // accept table headers or parent-table replacements as leaf-key aliases.
    if path.join(".") != key {
        return Err("resident Codex config requires a plain supported leaf key=value".into());
    }
    let supported_type = match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["sandbox_mode"] => value.as_str().is_some_and(|value| {
            matches!(
                value,
                "read-only" | "workspace-write" | "danger-full-access"
            )
        }),
        ["approval_policy"] => value.as_str().is_some_and(|value| {
            matches!(value, "untrusted" | "on-failure" | "on-request" | "never")
        }),
        ["agents", "enabled"]
        | ["features", "multi_agent_v2"]
        | ["features", "multi_agent"]
        | ["sandbox_workspace_write", "network_access"]
        | ["sandbox_workspace_write", "exclude_slash_tmp"]
        | ["sandbox_workspace_write", "exclude_tmpdir_env_var"] => value.is_bool(),
        ["sandbox_workspace_write", "writable_roots"] => value
            .as_array()
            .is_some_and(|values| values.iter().all(toml::Value::is_str)),
        _ => {
            return Err(format!(
                "Codex config key {:?} is unsupported by resident sessions; model/session/transport settings use session.open fields",
                path.join(".")
            ));
        }
    };
    if !supported_type {
        return Err(format!(
            "invalid value for resident Codex config key {:?}",
            path.join(".")
        ));
    }
    Ok(())
}
