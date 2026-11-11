//! Resolve existing JSON profiles at the resident-session boundary.

use std::{fs::OpenOptions as FileOptions, io::Read, path::PathBuf};

use serde::{Deserialize, Deserializer};

use crate::request::Request;
use crate::session::OpenOptions;
use due::types::{Effort, Provider};

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
