use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const SKILLS: &[&str] = &[
    "run",
    "contracts",
    "observe",
    #[cfg(feature = "harness")]
    "supervise",
];

struct Standalone {
    root: TempDir,
    binary: PathBuf,
}

impl Standalone {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let binary = root
            .path()
            .join(format!("relocated-due{}", std::env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_due"), &binary).unwrap();
        for directory in ["cwd", "tmp"] {
            fs::create_dir(root.path().join(directory)).unwrap();
        }
        Self { root, binary }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(&self.binary)
            .args(args)
            .current_dir(self.root.path().join("cwd"))
            .env_clear()
            .env("PATH", "")
            .env("TMPDIR", self.root.path().join("tmp"))
            .env("TMP", self.root.path().join("tmp"))
            .env("TEMP", self.root.path().join("tmp"))
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn assert_no_files_written(&self) {
        for directory in ["cwd", "tmp"] {
            assert_eq!(
                fs::read_dir(self.root.path().join(directory))
                    .unwrap()
                    .count(),
                0,
                "skills wrote files to {directory}"
            );
        }
        assert_eq!(fs::read_dir(self.root.path()).unwrap().count(), 3);
    }
}

fn describe(output: &Output) -> String {
    format!(
        "status={:?}, stdout={}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn success(output: &Output) {
    assert!(output.status.success(), "{}", describe(output));
    assert!(output.stderr.is_empty(), "{}", describe(output));
}

fn envelope(output: &Output, code: i32) -> Value {
    assert_eq!(output.status.code(), Some(code), "{}", describe(output));
    let value: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON: {error}; {}", describe(output)));
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["cli_version"], env!("CARGO_PKG_VERSION"));
    value
}

fn canonical(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("embedded-skills")
            .join(name)
            .join("SKILL.md"),
    )
    .unwrap()
}

#[test]
fn relocated_binary_lists_embedded_skills_without_provider_or_assets() {
    let standalone = Standalone::new();
    let mut catalogs = Vec::new();
    for args in [
        vec!["skills", "--output", "json"],
        vec!["skills", "list", "--output", "json"],
        vec!["skills", "--output", "json", "list"],
    ] {
        let output = standalone.run(&args);
        success(&output);
        let catalog = envelope(&output, 0);
        let skills = catalog["skills"].as_array().unwrap();
        let names: BTreeSet<_> = skills
            .iter()
            .map(|skill| {
                assert!(!skill["description"].as_str().unwrap().trim().is_empty());
                skill["name"].as_str().unwrap()
            })
            .collect();
        assert_eq!(skills.len(), SKILLS.len());
        assert_eq!(names, SKILLS.iter().copied().collect());
        catalogs.push(catalog);
    }
    assert!(catalogs.windows(2).all(|pair| pair[0] == pair[1]));
    standalone.assert_no_files_written();
}

#[test]
fn text_catalog_lists_names_and_descriptions_and_defaults_to_list() {
    let standalone = Standalone::new();
    let default = standalone.run(&["skills"]);
    let explicit = standalone.run(&["skills", "list", "--output", "text"]);
    success(&default);
    success(&explicit);
    assert_eq!(default.stdout, explicit.stdout);
    let text = String::from_utf8(default.stdout).unwrap();
    let catalog = envelope(&standalone.run(&["skills", "--output", "json"]), 0);
    for skill in catalog["skills"].as_array().unwrap() {
        assert!(text.contains(skill["name"].as_str().unwrap()));
        assert!(text.contains(skill["description"].as_str().unwrap()));
    }
    standalone.assert_no_files_written();
}

#[test]
fn show_returns_complete_canonical_documents_in_text_and_json() {
    let standalone = Standalone::new();
    for &name in SKILLS {
        let expected = canonical(name);
        let frontmatter = expected
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .expect("embedded skill must have YAML frontmatter");
        assert!(
            frontmatter
                .0
                .lines()
                .any(|line| line == format!("name: {name}"))
        );
        assert!(frontmatter.0.lines().any(|line| {
            line.strip_prefix("description:")
                .is_some_and(|description| !description.trim().is_empty())
        }));
        assert!(!frontmatter.1.trim().is_empty());

        let output = standalone.run(&["skills", "show", name]);
        success(&output);
        assert_eq!(output.stdout, expected.as_bytes());
        for args in [
            vec!["skills", "show", name, "--output", "json"],
            vec!["skills", "--output", "json", "show", name],
        ] {
            let output = standalone.run(&args);
            success(&output);
            let value = envelope(&output, 0);
            assert_eq!(value["skill"]["name"], name);
            assert_eq!(value["skill"]["content"], expected);
            assert!(
                !value["skill"]["description"]
                    .as_str()
                    .unwrap()
                    .trim()
                    .is_empty()
            );
        }
    }
    standalone.assert_no_files_written();
}

#[test]
fn unknown_skill_uses_output_specific_error_contract() {
    let standalone = Standalone::new();
    let output = standalone.run(&["skills", "show", "missing-skill"]);
    assert_eq!(output.status.code(), Some(2), "{}", describe(&output));
    assert!(output.stdout.is_empty(), "{}", describe(&output));
    assert!(!output.stderr.is_empty(), "{}", describe(&output));

    let output = standalone.run(&["skills", "show", "missing-skill", "--output", "json"]);
    let value = envelope(&output, 2);
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(
        !value["error"]["message"]
            .as_str()
            .unwrap()
            .trim()
            .is_empty()
    );
    assert!(value.get("skill").is_none());
    standalone.assert_no_files_written();
}

#[test]
fn skills_reject_invalid_flags_subcommands_and_missing_names() {
    let standalone = Standalone::new();
    for args in [
        vec!["skills", "--unknown-option"],
        vec!["skills", "list", "--unknown-option"],
        vec!["skills", "show", "run", "--unknown-option"],
        vec!["skills", "show"],
        vec!["skills", "unknown-command"],
        vec!["skills", "--output", "unsupported"],
    ] {
        let output = standalone.run(&args);
        assert_eq!(output.status.code(), Some(2), "{}", describe(&output));
        assert!(output.stdout.is_empty(), "{}", describe(&output));
        assert!(!output.stderr.is_empty(), "{}", describe(&output));
    }
    standalone.assert_no_files_written();
}

#[test]
fn help_exposes_run_and_skills_routes_without_a_provider() {
    let standalone = Standalone::new();
    for (args, discoverable) in [
        (vec!["--help"], vec!["run", "skills"]),
        (vec!["skills", "--help"], vec!["list", "show", "--output"]),
        (vec!["skills", "show", "--help"], vec!["show", "--output"]),
        (
            vec!["run", "--help"],
            vec!["--provider", "--schema", "--trace"],
        ),
    ] {
        let output = standalone.run(&args);
        success(&output);
        let help = String::from_utf8(output.stdout).unwrap();
        for command in discoverable {
            assert!(
                help.contains(command),
                "help did not expose {command}: {help}"
            );
        }
    }
    standalone.assert_no_files_written();
}

#[test]
fn standalone_run_guide_profile_matches_the_example() {
    let standalone = Standalone::new();
    let output = standalone.run(&["skills", "show", "run"]);
    success(&output);
    let guide = String::from_utf8(output.stdout).unwrap();
    let profile_section = guide
        .split_once("For persistent unattended Codex work, save this exact JSON as")
        .expect("run guide must teach the persistent profile")
        .1;
    let profile_json = profile_section
        .split_once("```json\n")
        .and_then(|(_, rest)| rest.split_once("\n```"))
        .expect("run guide must include standalone profile JSON")
        .0;
    let embedded: Value = serde_json::from_str(profile_json).unwrap();
    let example: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/codex-workspace-profile.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(embedded, example);
    standalone.assert_no_files_written();
}
