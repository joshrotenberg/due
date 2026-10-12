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
    #[cfg(feature = "workflow-fennel-spike")]
    Skill {
        name: "workflow-fennel",
        description: "Author an experimental Fennel function over the optional Lua workflow runtime with bounded offline compilation.",
        content: include_str!("../embedded-skills/workflow-fennel/SKILL.md"),
    },
    #[cfg(any(feature = "workflow-spike", feature = "workflow-lua-spike"))]
    Skill {
        name: "workflow",
        description: "Author and check an experimental bounded Lua or Rhai workflow with caller-declared Due profiles, schema gates, and private subprocess receipts.",
        content: include_str!("../embedded-skills/workflow/SKILL.md"),
    },
    Skill {
        name: "run",
        description: "Run one bounded Claude Code or Codex task through Due and independently verify its artifacts and effects.",
        content: include_str!("../embedded-skills/run/SKILL.md"),
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
