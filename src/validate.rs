use crate::{
    cli::{Provider, RunArgs},
    result::RunError,
};

/// Validate only the frontend-owned boundary; the provider owns other option syntax.
pub fn native_args(run: &RunArgs) -> Result<(), RunError> {
    let args = &run.native_args;
    if args.iter().any(|arg| arg == "--") {
        return Err(RunError::invalid(
            "a second -- would change the provider's prompt parsing",
        ));
    }
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if !token.starts_with('-') || token == "-" {
            return Err(RunError::invalid(format!(
                "unexpected native positional {token:?}; pass the prompt with --prompt or --stdin"
            )));
        }
        let name = token.split('=').next().unwrap_or(token);
        let conflict = match run.provider {
            Provider::Claude => claude_conflict(run, name, token),
            Provider::Codex => codex_conflict(run, name, token),
        };
        if let Some(reason) = conflict {
            return Err(RunError::invalid(format!("native option {name} {reason}")));
        }

        if run.provider == Provider::Codex
            && !token.starts_with("--")
            && let Some((letter, offset)) =
                token.char_indices().skip(1).find_map(|(offset, letter)| {
                    "cimps"
                        .contains(letter)
                        .then_some((letter, offset + letter.len_utf8()))
                })
        {
            let attached = token[offset..]
                .strip_prefix('=')
                .unwrap_or(&token[offset..]);
            let value = if attached.is_empty() {
                index += 1;
                args.get(index)
                    .ok_or_else(|| RunError::invalid(format!("native -{letter} needs a value")))?
            } else {
                attached
            };
            if letter == 'c' {
                check_config(run, value)?;
            }
            index += 1;
            continue;
        }
        if let Some((_, value)) = token.split_once('=') {
            if run.provider == Provider::Codex && name == "--config" {
                check_config(run, value)?;
            }
            index += 1;
            continue;
        }
        match arity(run.provider, name) {
            Arity::One => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| RunError::invalid(format!("native {name} needs a value")))?;
                if run.provider == Provider::Codex && name == "--config" {
                    check_config(run, value)?;
                }
            }
            Arity::Many => {
                while args
                    .get(index + 1)
                    .is_some_and(|value| !value.starts_with('-'))
                {
                    index += 1;
                }
            }
            Arity::Optional => {
                if args
                    .get(index + 1)
                    .is_some_and(|value| !value.starts_with('-'))
                {
                    index += 1;
                }
            }
            Arity::Zero => (),
        }
        index += 1;
    }
    Ok(())
}

fn claude_conflict(run: &RunArgs, name: &str, token: &str) -> Option<&'static str> {
    match name {
        "--print"
        | "--output-format"
        | "--input-format"
        | "--include-partial-messages"
        | "--replay-user-messages"
        | "--prompt-suggestions" => Some("is reserved for due's prompt/output transport"),
        "--resume"
        | "--continue"
        | "--session-id"
        | "--fork-session"
        | "--from-pr"
        | "--no-session-persistence" => Some("is reserved; use --resume or --ephemeral before --"),
        "--help" | "--version" => Some("does not run a model turn"),
        "--model" if run.model.is_some() => Some("conflicts with --model"),
        "--effort" if run.effort.is_some() => Some("conflicts with --effort"),
        "--json-schema" if run.schema.is_some() => Some("conflicts with --schema"),
        _ => {
            if !token.starts_with("--") {
                for letter in token[1..].chars() {
                    match letter {
                        'd' | 'w' => break, // The rest can be an attached option value.
                        'p' | 'r' | 'c' | 'h' | 'v' => {
                            return Some(
                                "contains a reserved prompt, session, help, or version option",
                            );
                        }
                        _ => (),
                    }
                }
            }
            None
        }
    }
}

fn codex_conflict(run: &RunArgs, name: &str, token: &str) -> Option<&'static str> {
    match name {
        "--json" | "--output-last-message" => Some("is reserved for due's output transport"),
        "--last" | "--all" | "--ephemeral" | "--session-id" | "--resume" => {
            Some("is reserved; use --resume or --ephemeral before --")
        }
        "--help" | "--version" => Some("does not run a model turn"),
        "--model" if run.model.is_some() => Some("conflicts with --model"),
        "--output-schema" if run.schema.is_some() => Some("conflicts with --schema"),
        "--cd" => Some("is reserved; use --cwd before --"),
        _ => {
            if token.starts_with("--") || !token.starts_with('-') {
                return None;
            }
            // -c's value may be attached and contain any letters; inspect it
            // separately as TOML rather than interpreting those letters as flags.
            for letter in token[1..].chars() {
                match letter {
                    'c' | 's' | 'i' | 'p' => break,
                    'm' if run.model.is_some() => return Some("conflicts with --model"),
                    'm' => break,
                    'C' => return Some("is reserved; use --cwd before --"),
                    'o' => return Some("is reserved for due's final-message output"),
                    'h' | 'V' => return Some("does not run a model turn"),
                    _ => (),
                }
            }
            None
        }
    }
}

enum Arity {
    Zero,
    One,
    Many,
    Optional,
}

fn arity(provider: Provider, name: &str) -> Arity {
    match (provider, name) {
        (
            Provider::Claude,
            "--model"
            | "--effort"
            | "--system-prompt"
            | "--append-system-prompt"
            | "--system-prompt-file"
            | "--append-system-prompt-file"
            | "--permission-mode"
            | "--max-turns"
            | "--max-budget-usd"
            | "--json-schema"
            | "--agent"
            | "--agents"
            | "--fallback-model"
            | "--settings"
            | "--setting-sources"
            | "--debug-file"
            | "--name"
            | "--plugin-dir"
            | "--plugin-url",
        ) => Arity::One,
        (
            Provider::Claude,
            "--allowed-tools" | "--allowedTools" | "--disallowed-tools" | "--disallowedTools"
            | "--tools" | "--add-dir" | "--mcp-config" | "--file" | "--betas",
        ) => Arity::Many,
        (
            Provider::Claude,
            "--verbose"
            | "--brief"
            | "--bare"
            | "--safe-mode"
            | "--tmux"
            | "--strict-mcp-config"
            | "--disable-slash-commands"
            | "--include-hook-events"
            | "--exclude-dynamic-system-prompt-sections"
            | "--dangerously-skip-permissions"
            | "--allow-dangerously-skip-permissions",
        ) => Arity::Zero,
        (
            Provider::Codex,
            "--model" | "--config" | "--image" | "--sandbox" | "--profile" | "--local-provider"
            | "--add-dir" | "--output-schema" | "--color" | "--enable" | "--disable",
        ) => Arity::One,
        (
            Provider::Codex,
            "--oss"
            | "--strict-config"
            | "--ignore-user-config"
            | "--ignore-rules"
            | "--full-auto"
            | "--approve-for-me"
            | "--skip-git-repo-check"
            | "--dangerously-bypass-approvals-and-sandbox"
            | "--yolo"
            | "--dangerously-bypass-hook-trust",
        ) => Arity::Zero,
        // Unknown flags retain an escape hatch without requiring this frontend to
        // track every release. A flag-looking value must use --name=value.
        _ => Arity::Optional,
    }
}

fn check_config(run: &RunArgs, value: &str) -> Result<(), RunError> {
    let Some((key, _)) = value.split_once('=') else {
        return Ok(()); // Leave malformed native configuration to the provider.
    };
    // Parsing a dummy value handles TOML quoted/dotted keys without needing to
    // understand Codex's fallback-to-string behavior for the actual value.
    let table = format!("{} = 0", key.trim()).parse::<toml::Table>();
    let Ok(table) = table else { return Ok(()) };
    for key in table.keys() {
        let conflict = match key.as_str() {
            "model" => run.model.is_some(),
            "model_reasoning_effort" => run.effort.is_some(),
            "output_schema" | "json_schema" => run.schema.is_some(),
            "cwd" => true,
            "prompt"
            | "input_format"
            | "output_format"
            | "output_last_message"
            | "json"
            | "session_id"
            | "thread_id"
            | "ephemeral" => true,
            _ => false,
        };
        if conflict {
            return Err(RunError::invalid(format!(
                "native configuration key {key} conflicts with due's managed options"
            )));
        }
    }
    Ok(())
}
