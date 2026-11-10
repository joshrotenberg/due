use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

// Copying a new executable must not overlap a fork/exec in another test thread.
// CLOEXEC closes an inherited writable handle at exec, not at fork; that brief
// inheritance can make Linux reject execution of the copy with ETXTBSY.
static COPY_OR_SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn capture(command: &mut Command) -> Output {
    let child = {
        let _guard = COPY_OR_SPAWN.lock().unwrap();
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    // The spawn has completed exec before releasing the lock. Process execution
    // and output collection can still proceed concurrently with other tests.
    child.wait_with_output().unwrap()
}

const SKILLS: &[&str] = &[
    #[cfg(feature = "mcp-stdio")]
    "mcp",
    "overview",
    "use-cases",
    "profiles",
    "run",
    "contracts",
    "observe",
    #[cfg(feature = "rpc")]
    "rpc",
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
        {
            let _guard = COPY_OR_SPAWN.lock().unwrap();
            fs::copy(env!("CARGO_BIN_EXE_due"), &binary).unwrap();
        }
        for directory in ["cwd", "tmp"] {
            fs::create_dir(root.path().join(directory)).unwrap();
        }
        Self { root, binary }
    }

    fn run(&self, args: &[&str]) -> Output {
        capture(
            Command::new(&self.binary)
                .args(args)
                .current_dir(self.root.path().join("cwd"))
                .env_clear()
                .env("PATH", "")
                .env("TMPDIR", self.root.path().join("tmp"))
                .env("TMP", self.root.path().join("tmp"))
                .env("TEMP", self.root.path().join("tmp"))
                .stdin(Stdio::null()),
        )
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

/// Every `due skills show NAME` route in the shipped guides and the plugin entry
/// skill must name a guide in the catalog. Reduced builds intentionally mention
/// feature-gated guides conditionally, so the check runs on the full build.
#[cfg(all(feature = "rpc", feature = "harness"))]
#[test]
fn guide_routes_name_catalog_entries() {
    let mut documents: Vec<(String, String)> = SKILLS
        .iter()
        .map(|&name| (format!("embedded-skills/{name}"), canonical(name)))
        .collect();
    documents.push((
        "plugin/due/skills/delegate".into(),
        fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin/due/skills/delegate/SKILL.md"),
        )
        .unwrap(),
    ));
    let mut routes = 0;
    for (source, text) in &documents {
        for rest in text.split("due skills show ").skip(1) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                .collect();
            if name.is_empty() {
                continue; // Placeholder such as `NAME`.
            }
            routes += 1;
            assert!(
                SKILLS.contains(&name.as_str()),
                "{source} routes to unknown guide {name:?}"
            );
        }
    }
    assert!(routes > 0, "no guide routes found");
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
        if args == ["--help"] {
            assert_eq!(
                help.contains("due skills show mcp"),
                cfg!(feature = "mcp-stdio")
            );
        }
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
fn standalone_profile_guide_matches_the_workspace_example() {
    let standalone = Standalone::new();
    let output = standalone.run(&["skills", "show", "profiles"]);
    success(&output);
    let guide = String::from_utf8(output.stdout).unwrap();
    let embedded: Value = guide
        .split("```json\n")
        .skip(1)
        .filter_map(|block| block.split_once("\n```").map(|(json, _)| json))
        .filter_map(|json| serde_json::from_str::<Value>(json).ok())
        .find(|value| value["provider"] == "codex" && value["native_args"].is_array())
        .expect("profile guide must ship the full workspace profile");
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

#[test]
fn short_and_long_help_are_distinct_offline_for_each_command() {
    let root = tempfile::tempdir().unwrap();
    let mut commands: Vec<Vec<&str>> = vec![
        vec![],
        vec!["run"],
        vec!["init"],
        vec!["skills"],
        vec!["skills", "list"],
        vec!["skills", "show"],
        vec!["schema"],
        vec!["trace"],
        vec!["trace", "summarize"],
    ];
    if cfg!(feature = "rpc") {
        commands.push(vec!["rpc"]);
    }
    if cfg!(feature = "harness") {
        commands.push(vec!["script"]);
    }
    if cfg!(feature = "mcp-stdio") {
        commands.push(vec!["mcp"]);
    }
    for command in commands {
        let invoke = |help: &str| {
            capture(
                Command::new(env!("CARGO_BIN_EXE_due"))
                    .args(&command)
                    .arg(help)
                    .current_dir(root.path())
                    .env_clear()
                    .env("PATH", "")
                    .stdin(Stdio::null()),
            )
        };
        let short = invoke("-h");
        let long = invoke("--help");
        success(&short);
        success(&long);
        assert!(
            long.stdout.len() > short.stdout.len(),
            "long help adds no detail for {command:?}"
        );
        assert_ne!(short.stdout, long.stdout, "identical help for {command:?}");
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
