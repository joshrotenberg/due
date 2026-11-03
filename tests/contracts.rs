use serde_json::{Value, json};
use std::process::{Command, Stdio};

fn exported(name: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_due"))
        .args(["schema", name])
        .env("PATH", "")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[cfg(feature = "harness")]
#[test]
fn script_result_contract_covers_success_and_input_failure() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("result.js");
    std::fs::write(
        &script,
        "Due.run(async scope => ({accepted:false, checks:[]}));",
    )
    .unwrap();
    let schema = exported("script-result");
    for (name, args, code) in [("ok", "{}", 0), ("bad", "not-json", 2)] {
        let output = Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("script")
            .arg(&script)
            .arg("--evidence")
            .arg(root.path().join(name))
            .args(["--args", args])
            .env("PATH", "")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(matches(&schema, &value, &schema), "{value}");
        if code == 0 {
            // Successful execution does not reinterpret a caller's verdict.
            assert_eq!(value["result"]["accepted"], false);
        } else {
            let mut invalid = value.clone();
            invalid["result"] = json!({"accepted":true});
            assert!(!matches(&schema, &invalid, &schema));
            assert!(!root.path().join(name).exists());
        }
    }
}

// Check the exported schema's structural assertions against actual CLI output.
// This test helper is deliberately not an independent model-answer validator.
fn matches(schema: &Value, value: &Value, root: &Value) -> bool {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return matches(
            root.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
            value,
            root,
        );
    }
    if let Some(constant) = schema.get("const")
        && constant != value
    {
        return false;
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array)
        && !choices.contains(value)
    {
        return false;
    }
    let is_type = |name: &str| match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_u64() || value.is_i64(),
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => panic!("unknown schema type {name}"),
    };
    if let Some(kind) = schema.get("type") {
        let valid = kind.as_str().map(is_type).unwrap_or_else(|| {
            kind.as_array()
                .unwrap()
                .iter()
                .any(|k| is_type(k.as_str().unwrap()))
        });
        if !valid {
            return false;
        }
    }
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|n| n < minimum)
    {
        return false;
    }
    if let Some(text) = value.as_str() {
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|n| text.chars().count() < n as usize)
        {
            return false;
        }
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|n| text.chars().count() > n as usize)
        {
            return false;
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array)
            && required
                .iter()
                .any(|k| !object.contains_key(k.as_str().unwrap()))
        {
            return false;
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, field) in object {
            if let Some(property) = properties.and_then(|p| p.get(key)) {
                if !matches(property, field, root) {
                    return false;
                }
            } else if let Some(additional) = schema.get("additionalProperties")
                && (additional == &Value::Bool(false)
                    || (additional.is_object() && !matches(additional, field, root)))
            {
                return false;
            }
            if let Some(names) = schema.get("propertyNames")
                && !matches(names, &Value::String(key.clone()), root)
            {
                return false;
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array())
        && values.iter().any(|v| !matches(items, v, root))
    {
        return false;
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
            let count = branches.iter().filter(|s| matches(s, value, root)).count();
            if match keyword {
                "allOf" => count != branches.len(),
                "anyOf" => count == 0,
                _ => count != 1,
            } {
                return false;
            }
        }
    }
    if let Some(negated) = schema.get("not")
        && matches(negated, value, root)
    {
        return false;
    }
    if let Some(condition) = schema.get("if") {
        let branch = if matches(condition, value, root) {
            "then"
        } else {
            "else"
        };
        if let Some(selected) = schema.get(branch)
            && !matches(selected, value, root)
        {
            return false;
        }
    }
    true
}

#[test]
fn schema_exports_are_offline_embedded_documents() {
    for (name, bundled) in [
        (
            "request",
            include_str!("../schemas/run-request.schema.json"),
        ),
        ("result", include_str!("../schemas/run-result.schema.json")),
        ("trace", include_str!("../schemas/trace.schema.json")),
        (
            "trace-summary",
            include_str!("../schemas/trace-summary.schema.json"),
        ),
        #[cfg(feature = "harness")]
        (
            "script-result",
            include_str!("../schemas/script-result.schema.json"),
        ),
    ] {
        let schema = exported(name);
        assert_eq!(schema, serde_json::from_str::<Value>(bundled).unwrap());
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
    }
    let request = exported("request");
    assert!(
        matches(&request, &json!({"schema_version":1}), &request),
        "profiles may be partial"
    );
    assert!(!matches(
        &request,
        &json!({"schema_version":1,"provider":null}),
        &request
    ));
    let invalid = Command::new(env!("CARGO_BIN_EXE_due"))
        .args(["schema", "missing"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
}

#[test]
fn result_schema_accepts_saved_v1_without_provenance() {
    let schema = exported("result");
    let mut saved: Value = serde_json::from_str(include_str!(
        "../examples/offline-validation/fixtures/approve.result.json"
    ))
    .unwrap();
    assert!(saved.get("provenance").is_none());
    assert!(matches(&schema, &saved, &schema));
    // Optional does not relax the shape when a record supplies provenance.
    saved["provenance"] = json!({"requested":null});
    assert!(!matches(&schema, &saved, &schema));
}

#[test]
fn result_schema_covers_plans_and_input_errors_and_rejects_broken_presence() {
    let schema = exported("result");
    for args in [
        vec![
            "run",
            "--provider",
            "codex",
            "--prompt",
            "hello",
            "--output",
            "json",
            "--dry-run",
        ],
        vec![
            "run",
            "--request",
            "/nonexistent-due-contract-request",
            "--output",
            "json",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_due"))
            .args(args)
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(result["provenance"].is_object());
        assert!(matches(&schema, &result, &schema), "result={result}");
        let mut broken = result.clone();
        broken["structured_output_present"] = json!(false);
        broken["structured_output"] = json!({"unexpected":true});
        assert!(!matches(&schema, &broken, &schema));
        let mut broken = result.clone();
        broken
            .as_object_mut()
            .unwrap()
            .remove("answer_schema_validation");
        assert!(!matches(&schema, &broken, &schema));
        broken = result;
        broken["answer_schema_validation"] = json!("validated");
        assert!(!matches(&schema, &broken, &schema));
    }
}

#[cfg(unix)]
#[test]
fn actual_success_failure_and_trace_records_match_exported_contracts() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let result_schema = exported("result");
    let trace_schema = exported("trace");
    let summary_schema = exported("trace-summary");
    for provider in ["claude", "codex"] {
        for mode in ["success", "failure_null", "missing"] {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("schema-provider");
            fs::write(&binary, include_bytes!("fixtures/schema-agent.sh")).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
            let answer_schema = dir.path().join("answer.json");
            fs::write(&answer_schema, "true").unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_due"))
                .args([
                    "run",
                    "--provider",
                    provider,
                    "--prompt",
                    "hello",
                    "--output",
                    "json",
                    "--trace=json",
                    "--binary",
                ])
                .arg(&binary)
                .arg("--schema")
                .arg(&answer_schema)
                .env("DUE_SCHEMA_DIR", dir.path())
                .env("DUE_SCHEMA_PROVIDER", provider)
                .env("DUE_SCHEMA_MODE", mode)
                .env("DUE_SCHEMA_PAYLOAD", "null")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(
                matches(&result_schema, &result, &result_schema),
                "{provider}/{mode}: {result}"
            );
            assert_eq!(result["structured_output_present"], mode != "missing");
            let trace_text = String::from_utf8(output.stderr).unwrap();
            for line in trace_text.lines() {
                let record: Value = serde_json::from_str(line).unwrap();
                assert!(matches(&trace_schema, &record, &trace_schema), "{record}");
                let mut leaked = record;
                leaked["prompt"] = json!("content");
                assert!(!matches(&trace_schema, &leaked, &trace_schema));
            }
            let path = dir.path().join("trace.jsonl");
            fs::write(&path, trace_text).unwrap();
            let summarized = Command::new(env!("CARGO_BIN_EXE_due"))
                .args(["trace", "summarize"])
                .arg(path)
                .args(["--output", "json"])
                .output()
                .unwrap();
            assert!(summarized.status.success());
            let summary: Value = serde_json::from_slice(&summarized.stdout).unwrap();
            assert!(
                matches(&summary_schema, &summary, &summary_schema),
                "{summary}"
            );
            assert_eq!(summary["runs"][0]["status"], result["status"]);
        }
    }
}

#[test]
fn rejected_intake_preserves_known_schema_intent_without_guessing_from_broken_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let malformed = dir.path().join("broken.json");
    std::fs::write(
        &malformed,
        r#"{"schema_version":1,"schema":"answer.json",broken"#,
    )
    .unwrap();
    for path in [dir.path().join("missing.json"), malformed] {
        for explicit in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
            command
                .args(["run", "--request"])
                .arg(&path)
                .args(["--output", "json"]);
            if explicit {
                command.args(["--schema", "answer.json"]);
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(2));
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(result["provider"].is_null());
            assert_eq!(
                result["answer_schema_validation"],
                if explicit {
                    "not_performed"
                } else {
                    "not_resolved"
                }
            );
            assert_eq!(result["structured_output_present"], false);
        }
    }
}

#[test]
fn trace_summary_contract_covers_empty_trace_and_both_error_kinds() {
    let schema = exported("trace-summary");
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.jsonl");
    let malformed = dir.path().join("malformed.jsonl");
    std::fs::write(&empty, "").unwrap();
    std::fs::write(&malformed, "not JSON\n").unwrap();
    for (path, exit, kind) in [
        (empty, 0, None),
        (malformed, 2, Some("invalid_input")),
        (dir.path().join("missing.jsonl"), 1, Some("io")),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_due"))
            .args(["trace", "summarize"])
            .arg(path)
            .args(["--output", "json"])
            .env("PATH", "")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(exit));
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(matches(&schema, &value, &schema), "{value}");
        if let Some(kind) = kind {
            assert_eq!(value["error"]["kind"], kind);
        } else {
            assert_eq!(value["records"], 0);
            assert_eq!(value["runs"], json!([]));
        }
    }
}
