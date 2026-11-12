//! Offline skill discovery. All instructions ship inside the installed binary.

use std::{
    io::{self, Write},
    process::ExitCode,
};

use serde::Serialize;
use serde_json::json;

use crate::cli::{Output, SkillsArgs, SkillsCommand};

#[derive(Serialize)]
struct Skill {
    name: &'static str,
    description: &'static str,
    #[serde(skip_serializing)]
    content: &'static str,
}

const SKILLS: &[Skill] = &[
    #[cfg(feature = "mcp-stdio")]
    Skill {
        name: "mcp",
        description: "Configure a stdio MCP client to start Due workers, follow up, read session items, and reply to approvals.",
        content: include_str!("../embedded-skills/mcp/SKILL.md"),
    },
    Skill {
        name: "overview",
        description: "Choose between Due's one-shot run, resident script and raw RPC modes, get started offline, discover installed help and guides, and troubleshoot or report problems.",
        content: include_str!("../embedded-skills/overview/SKILL.md"),
    },
    Skill {
        name: "use-cases",
        description: "Decide whether a task suits Due and match it to a pattern (bounded delegation, multi-turn specialist, review and revise, independent assignments, structured extraction, embedding in a program), with one provider or two.",
        content: include_str!("../embedded-skills/use-cases/SKILL.md"),
    },
    Skill {
        name: "work-loop",
        description: "Carry out an order larger than one assignment (a backlog of issues, one hard change with independent review, recurring upkeep) by splitting it into pieces, running each piece through Due workers, checking every result, and asking the person only for decisions that cannot be undone or are out of scope.",
        content: include_str!("../embedded-skills/work-loop/SKILL.md"),
    },
    Skill {
        name: "run",
        description: "Scope, execute and independently verify one bounded Claude Code or Codex assignment, with optional native-session resume.",
        content: include_str!("../embedded-skills/run/SKILL.md"),
    },
    Skill {
        name: "profiles",
        description: "Write and reuse versioned JSON profiles across due run, due script and raw RPC, including precedence, path rules and which native options each surface accepts.",
        content: include_str!("../embedded-skills/profiles/SKILL.md"),
    },
    Skill {
        name: "contracts",
        description: "Request structured answers with Due and interpret its result envelope, typed failures, and provider-specific limits.",
        content: include_str!("../embedded-skills/contracts/SKILL.md"),
    },
    Skill {
        name: "observe",
        description: "Observe a Due run through content-limited progress traces while preserving final results and detecting trace loss.",
        content: include_str!("../embedded-skills/observe/SKILL.md"),
    },
    #[cfg(feature = "rpc")]
    Skill {
        name: "rpc",
        description: "Write a raw stdio JSON-RPC client for a resident Claude Code or Codex session, with the method map, streaming items, interaction replies, retention limits, lifecycle and error semantics.",
        content: include_str!("../embedded-skills/rpc/SKILL.md"),
    },
    #[cfg(feature = "harness")]
    Skill {
        name: "supervise",
        description: "Supervise a bounded Claude Code or Codex assignment across resident turns with embedded JavaScript, streamed items, explicit permission decisions and independent verification.",
        content: include_str!("../embedded-skills/supervise/SKILL.md"),
    },
];

pub fn execute(args: &SkillsArgs) -> ExitCode {
    let selected = match &args.command {
        Some(SkillsCommand::Show { name }) => {
            match SKILLS.iter().find(|skill| skill.name == name) {
                Some(skill) => Some(skill),
                None => return unknown_skill(args.output, name),
            }
        }
        None | Some(SkillsCommand::List) => None,
    };
    let mut stdout = io::stdout().lock();
    let outcome = match args.output {
        Output::Json => {
            let document = match selected {
                Some(skill) => json!({
                    "schema_version": 1,
                    "cli_version": env!("CARGO_PKG_VERSION"),
                    "skill": {"name": skill.name, "description": skill.description, "content": skill.content}
                }),
                None => json!({
                    "schema_version": 1,
                    "cli_version": env!("CARGO_PKG_VERSION"),
                    "skills": SKILLS
                }),
            };
            write_json(&mut stdout, &document)
        }
        Output::Text => match selected {
            Some(skill) => stdout.write_all(skill.content.as_bytes()),
            None => SKILLS.iter().try_for_each(|skill| writeln!(stdout, "{}\t{}", skill.name, skill.description)),
        },
    }.and_then(|_| stdout.flush());
    output_status(outcome)
}

fn unknown_skill(output: Output, name: &str) -> ExitCode {
    let message =
        format!("unknown skill {name:?}; use `due skills list` to discover available names");
    let outcome = match output {
        Output::Json => {
            let mut stdout = io::stdout().lock();
            write_json(
                &mut stdout,
                &json!({
                    "schema_version": 1,
                    "cli_version": env!("CARGO_PKG_VERSION"),
                    "error": {"kind": "invalid_input", "message": message}
                }),
            )
            .and_then(|_| stdout.flush())
        }
        Output::Text => writeln!(io::stderr().lock(), "due: {message}"),
    };
    if outcome.is_err() {
        return output_status(outcome);
    }
    ExitCode::from(2)
}

fn write_json(writer: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writeln!(writer)
}

fn output_status(outcome: io::Result<()>) -> ExitCode {
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if error.kind() != io::ErrorKind::BrokenPipe {
                let _ = writeln!(
                    io::stderr().lock(),
                    "due: failed to write skill output: {error}"
                );
            }
            ExitCode::from(1)
        }
    }
}
