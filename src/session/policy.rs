//! Shared resident native-policy validation.

use crate::types::Provider;

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
