//! Offline checks for the source distribution and its skills-only plugin.
use serde_json::Value;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap()
}

fn contained(root: &Path, relative: &str) -> PathBuf {
    let path = Path::new(relative);
    assert!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::CurDir | Component::Normal(_))),
        "package path must be relative and contained: {relative:?}"
    );
    let resolved = root
        .join(path)
        .canonicalize()
        .unwrap_or_else(|error| panic!("missing package path {relative:?}: {error}"));
    assert!(
        resolved.starts_with(root),
        "package path escapes its root: {relative:?}"
    );
    resolved
}

fn document(root: &Path, relative: &str) -> String {
    let path = contained(root, relative);
    assert!(path.is_file(), "expected a packaged file: {relative}");
    let text = fs::read_to_string(path).unwrap();
    assert!(!text.trim().is_empty(), "empty packaged file: {relative}");
    text
}

fn json(root: &Path, relative: &str) -> Value {
    serde_json::from_str(&document(root, relative))
        .unwrap_or_else(|error| panic!("invalid package JSON {relative:?}: {error}"))
}

fn nonempty(value: &Value, field: &str) {
    assert!(
        value[field].as_str().is_some_and(|s| !s.trim().is_empty()),
        "missing metadata {field}"
    );
}

#[test]
fn plugin_identity_version_and_entry_skill_match_the_source_package() {
    let root = root();
    let cargo: toml::Value = toml::from_str(&document(&root, "Cargo.toml")).unwrap();
    let plugin_root = contained(&root, "plugin/due");
    let manifest = json(&plugin_root, "plugin.json");
    assert_eq!(manifest["name"].as_str(), cargo["package"]["name"].as_str());
    assert_eq!(
        manifest["version"].as_str(),
        cargo["package"]["version"].as_str()
    );
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        manifest["$schema"],
        "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
    );
    nonempty(&manifest, "description");
    nonempty(&manifest["author"], "name");
    let interface = &manifest["extensions"]["com.openai"]["interface"];
    for field in [
        "displayName",
        "shortDescription",
        "longDescription",
        "developerName",
        "defaultPrompt",
    ] {
        nonempty(interface, field);
    }
    assert!(
        interface["shortDescription"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 30
    );

    let skill = document(&plugin_root, "skills/delegate/SKILL.md");
    let (frontmatter, body) = skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .expect("entry skill needs YAML frontmatter and a body");
    assert!(frontmatter.lines().any(|line| line == "name: delegate"));
    assert!(frontmatter.lines().any(|line| {
        line.strip_prefix("description:")
            .is_some_and(|text| !text.trim().is_empty())
    }));
    assert!(!body.trim().is_empty());
}

#[test]
fn local_marketplace_resolves_the_real_contained_plugin() {
    let root = root();
    let marketplace = json(&root, ".agents/plugins/marketplace.json");
    assert_eq!(marketplace["name"], "due-dev");
    nonempty(&marketplace["interface"], "displayName");
    let entries = marketplace["plugins"]
        .as_array()
        .expect("marketplace plugins array");
    let matching: Vec<_> = entries
        .iter()
        .filter(|entry| entry["name"] == env!("CARGO_PKG_NAME"))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "the package needs one unambiguous marketplace entry"
    );
    let source = &matching[0]["source"];
    assert_eq!(source["source"], "local");
    let plugin_root = contained(
        &root,
        source["path"].as_str().expect("local plugin source path"),
    );
    assert!(plugin_root.is_dir());
    let manifest = json(&plugin_root, "plugin.json");
    assert_eq!(manifest["name"], matching[0]["name"]);
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    document(&plugin_root, "skills/delegate/SKILL.md");
}

fn assert_no_service_declarations(value: &Value) {
    match value {
        Value::Object(object) => {
            for (key, nested) in object {
                assert!(
                    !matches!(key.as_str(), "mcp" | "mcpServers" | "mcp_servers" | "hooks"),
                    "skills-only plugin unexpectedly declares {key}"
                );
                assert_no_service_declarations(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                assert_no_service_declarations(nested);
            }
        }
        _ => (),
    }
}

fn assert_no_service_payloads(directory: &Path) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        assert!(
            !matches!(
                name.to_str(),
                Some("hooks" | "hooks.json" | ".mcp.json" | "mcp.json" | "mcp-servers.json")
            ),
            "skills-only plugin includes an unexpected service payload: {:?}",
            entry.path()
        );
        // Package files should not secretly point outside the skills-only bundle.
        assert!(
            !entry.file_type().unwrap().is_symlink(),
            "plugin payload is a symlink: {:?}",
            entry.path()
        );
        if entry.file_type().unwrap().is_dir() {
            assert_no_service_payloads(&entry.path());
        }
    }
}

#[test]
fn source_distribution_has_its_docs_contracts_and_no_plugin_services() {
    let root = root();
    for path in [
        "README.md",
        "CONTRIBUTING.md",
        "docs/install.md",
        "docs/cli.md",
        "docs/dogfooding.md",
        "docs/harness.md",
        "docs/rpc.md",
        "embedded-skills/overview/SKILL.md",
        "embedded-skills/profiles/SKILL.md",
        "embedded-skills/rpc/SKILL.md",
        "embedded-skills/run/SKILL.md",
        "embedded-skills/contracts/SKILL.md",
        "embedded-skills/observe/SKILL.md",
        "embedded-skills/supervise/SKILL.md",
        "crates/due-harness/Cargo.toml",
        "crates/due-harness/src/lib.rs",
        "crates/due-harness/clients/async.js",
        "examples/harness/supervision.js",
    ] {
        document(&root, path);
    }
    for path in [
        "schemas/run-request.schema.json",
        "schemas/run-result.schema.json",
        "schemas/trace.schema.json",
        "schemas/trace-summary.schema.json",
        "schemas/script-result.schema.json",
    ] {
        let schema = json(&root, path);
        assert!(schema.is_object());
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
    }
    let plugin_root = contained(&root, "plugin/due");
    assert_no_service_declarations(&json(&plugin_root, "plugin.json"));
    assert_no_service_payloads(&plugin_root);
}
