#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    provider: &'static str,
}

impl Fixture {
    fn new(provider: &'static str) -> Self {
        Self::with_script(provider, include_bytes!("fixtures/fake-agent.sh"))
    }

    fn with_script(provider: &'static str, script: &[u8]) -> Self {
        let root = tempfile::tempdir().unwrap();
        for directory in ["bin", "capture", "requests", "project"] {
            fs::create_dir(root.path().join(directory)).unwrap();
        }
        let binary = root.path().join("bin/fake-provider");
        fs::write(&binary, script).unwrap();
        fs::set_permissions(binary, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, provider }
    }

    fn command(&self, mode: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .args(["run", "--binary", "bin/fake-provider"])
            .current_dir(self.root.path())
            .env("DUE_TEST_DIR", self.root.path().join("capture"))
            .env("DUE_TEST_PROVIDER", self.provider)
            .env("DUE_TEST_MODE", mode)
            .env("DUE_SCHEMA_DIR", self.root.path().join("capture"))
            .env("DUE_SCHEMA_PROVIDER", self.provider)
            .env("DUE_SCHEMA_MODE", mode)
            .env("DUE_SCHEMA_PAYLOAD", r#"{"answer":42,"ok":true}"#)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn request(&self, value: &Value) -> PathBuf {
        self.raw_request(&value.to_string())
    }

    fn raw_request(&self, contents: &str) -> PathBuf {
        let path = self.root.path().join("requests/turn.json");
        fs::write(&path, contents).unwrap();
        path
    }

    fn capture(&self, name: &str) -> Vec<u8> {
        fs::read(self.root.path().join("capture").join(name)).unwrap()
    }

    fn argv(&self) -> Vec<String> {
        let bytes = self.capture("argv");
        bytes
            .strip_suffix(&[0])
            .unwrap()
            .split(|byte| *byte == 0)
            .map(|part| String::from_utf8(part.to_vec()).unwrap())
            .collect()
    }

    fn was_spawned(&self) -> bool {
        self.root.path().join("capture/argv").exists()
    }
}

fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("request invocation exceeded the test deadline");
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

fn run(command: &mut Command) -> Output {
    finish(command.spawn().unwrap())
}

fn result(output: &Output, exit: i32, status: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // from_slice rejects extra JSON documents or diagnostics on stdout.
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], status);
    value
}

fn stable_result(mut result: Value) -> Value {
    result.as_object_mut().unwrap().remove("duration_ms");
    result
}

fn stable_argv(mut args: Vec<String>) -> Vec<String> {
    for index in 1..args.len() {
        match args[index - 1].as_str() {
            "--output-last-message" => args[index] = "<per-run-final-file>".into(),
            "--output-schema" => args[index] = "<per-run-schema-file>".into(),
            _ => (),
        }
    }
    args
}

#[test]
fn request_and_flag_runs_share_argv_prompt_and_result_contract() {
    let prompt = "Keep $HOME, $(touch should-not-exist), `false`, and ; literal.\n日本語\n";
    let native = ["--custom-one", "a b;$HOME", "--custom-two=literal"];
    for provider in ["claude", "codex"] {
        for session in ["fresh", "resume", "ephemeral"] {
            let fixture = Fixture::new(provider);
            let mut request = json!({
                "schema_version":1, "provider":provider, "prompt":prompt,
                "model":"native-model", "effort":"high", "cwd":"project",
                "timeout":"10s", "native_args":native
            });
            let mut flags = fixture.command("success");
            flags.args([
                "--provider",
                provider,
                "--prompt",
                prompt,
                "--model",
                "native-model",
                "--effort",
                "high",
                "--cwd",
                "project",
                "--timeout",
                "10s",
                "--output",
                "json",
            ]);
            match session {
                "resume" => {
                    request["resume"] = json!("known-session");
                    flags.args(["--resume", "known-session"]);
                }
                "ephemeral" => {
                    request["ephemeral"] = json!(true);
                    flags.arg("--ephemeral");
                }
                _ => (),
            }
            flags.arg("--").args(native);
            let expected = result(&run(&mut flags), 0, "succeeded");
            let expected_argv = stable_argv(fixture.argv());
            let path = fixture.request(&request);
            let actual = result(
                &run(fixture
                    .command("success")
                    .arg("--request")
                    .arg(path)
                    .args(["--output", "json"])),
                0,
                "succeeded",
            );
            assert_eq!(stable_result(actual), stable_result(expected));
            assert_eq!(stable_argv(fixture.argv()), expected_argv);
            assert_eq!(fixture.capture("stdin"), prompt.as_bytes());
            assert!(!fixture.argv().iter().any(|argument| argument == prompt));
            assert_eq!(
                String::from_utf8(fixture.capture("cwd")).unwrap().trim(),
                fixture
                    .root
                    .path()
                    .join("project")
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
            );
            assert!(
                !fixture
                    .root
                    .path()
                    .join("project/should-not-exist")
                    .exists()
            );
        }
    }
}

#[test]
fn stdin_request_decodes_once_and_default_output_remains_text() {
    let prompt = "first line\n--provider other\n$HOME $(touch not-a-command) `false`;\n\u{2603}\n";
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let request = json!({"schema_version":1,"provider":provider,"prompt":prompt});
        let mut child = fixture
            .command("success")
            .args(["--request", "-"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let output = finish(child);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"FINAL ANSWER\n");
        assert_eq!(fixture.capture("stdin"), prompt.as_bytes());
        assert!(!fixture.root.path().join("not-a-command").exists());
    }
}

#[test]
fn request_schema_paths_use_caller_cwd_and_match_flag_execution() {
    let schema = json!({"type":"object","properties":{"answer":{"type":"integer"}}});
    for provider in ["claude", "codex"] {
        let fixture = Fixture::with_script(provider, include_bytes!("fixtures/schema-agent.sh"));
        fs::write(
            fixture.root.path().join("answer.schema.json"),
            schema.to_string(),
        )
        .unwrap();
        for directory in ["requests", "project"] {
            fs::write(
                fixture
                    .root
                    .path()
                    .join(directory)
                    .join("answer.schema.json"),
                "false",
            )
            .unwrap();
        }
        let request = fixture.request(&json!({
            "schema_version":1,"provider":provider,"prompt":"structured answer",
            "cwd":"project","schema":"answer.schema.json","resume":"known-session"
        }));
        let expected = result(
            &run(fixture.command("success").args([
                "--provider",
                provider,
                "--prompt",
                "structured answer",
                "--cwd",
                "project",
                "--schema",
                "answer.schema.json",
                "--resume",
                "known-session",
                "--output",
                "json",
            ])),
            0,
            "succeeded",
        );
        let expected_argv = stable_argv(fixture.argv());
        let actual = result(
            &run(fixture.command("success").args([
                "--request",
                "requests/turn.json",
                "--output",
                "json",
            ])),
            0,
            "succeeded",
        );
        assert!(request.exists());
        assert_eq!(actual["structured_output"], json!({"answer":42,"ok":true}));
        assert_eq!(stable_result(actual), stable_result(expected));
        assert_eq!(stable_argv(fixture.argv()), expected_argv);
        assert_eq!(fixture.capture("stdin"), b"structured answer");
        assert_eq!(
            serde_json::from_slice::<Value>(&fixture.capture("schema-content")).unwrap(),
            schema
        );
    }
}

#[test]
fn request_allows_operational_flags_and_dry_run_without_spawning() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let path = fixture
            .request(&json!({"schema_version":1,"provider":provider,"prompt":"private prompt"}));
        let trace = fixture.root.path().join("progress.jsonl");
        let output = run(fixture
            .command("success")
            .arg("--request")
            .arg(path)
            .args([
                "--output",
                "json",
                "--dry-run",
                "--trace=json",
                "--trace-interval",
                "10ms",
                "--trace-file",
            ])
            .arg(&trace));
        let value = result(&output, 0, "planned");
        assert_eq!(value["provider"], provider);
        assert_eq!(value["invocation"]["stdin_bytes"], "private prompt".len());
        assert!(
            value["invocation"]["binary"]
                .as_str()
                .unwrap()
                .ends_with("bin/fake-provider")
        );
        assert!(!fixture.was_spawned());
        let records = fs::read_to_string(trace).unwrap();
        assert!(!records.contains("private prompt"));
        let records: Vec<Value> = records
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(
            records
                .iter()
                .any(|record| record["event"] == "run_finished" && record["status"] == "planned")
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .all(|line| serde_json::from_str::<Value>(line).is_ok())
        );
    }
}

#[test]
fn malformed_requests_fail_with_unknown_provider_before_trace_or_spawn() {
    let base = json!({"schema_version":1,"provider":"codex","prompt":"task"});
    let mut cases = vec![
        "{broken".to_owned(),
        "{}".to_owned(),
        format!("{base} {base}"),
        r#"{"schema_version":1,"provider":"codex","provider":"claude","prompt":"task"}"#.into(),
    ];
    for required in ["schema_version", "provider", "prompt"] {
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove(required);
        cases.push(missing.to_string());
    }
    for (key, replacement) in [
        ("schema_version", json!(2)),
        ("schema_version", json!("1")),
        ("provider", json!("unknown-provider")),
        ("provider", Value::Null),
        ("prompt", json!(42)),
        ("prompt", json!(" \n\t")),
        ("model", json!("")),
        ("model", json!(5)),
        ("resume", json!("")),
        ("effort", json!("extreme")),
        ("cwd", json!(17)),
        ("schema", json!(false)),
        ("timeout", json!("0s")),
        ("timeout", json!(10)),
        ("ephemeral", json!("true")),
        ("native_args", json!(["--flag", 17])),
        ("unexpected_key", json!("reject me")),
    ] {
        let mut value = base.clone();
        value[key] = replacement;
        cases.push(value.to_string());
    }
    // Keep the complete conflicting request as one intake validation case.
    let mut conflict = base;
    conflict["resume"] = json!("known-session");
    conflict["ephemeral"] = json!(true);
    cases.push(conflict.to_string());
    for contents in cases {
        let fixture = Fixture::new("codex");
        let path = fixture.raw_request(&contents);
        let trace = fixture.root.path().join("should-not-exist.jsonl");
        let value = result(
            &run(fixture
                .command("success")
                .arg("--request")
                .arg(path)
                .args(["--output", "json", "--trace=json", "--trace-file"])
                .arg(&trace)),
            2,
            "failed",
        );
        assert!(
            value["provider"].is_null(),
            "intake failure selected a provider: {contents}"
        );
        assert_eq!(value["error"]["kind"], "invalid_input");
        assert!(value["error"]["provider_exit_code"].is_null());
        for field in [
            "text",
            "structured_output",
            "session_id",
            "usage",
            "cost_usd",
            "duration_ms",
        ] {
            assert!(value.get(field).is_some(), "missing result field {field}");
        }
        assert!(!trace.exists(), "intake failure created trace: {contents}");
        assert!(
            !fixture.was_spawned(),
            "intake failure spawned provider: {contents}"
        );
    }
}

#[test]
fn unreadable_request_is_an_intake_failure() {
    let fixture = Fixture::new("codex");
    let output = run(fixture.command("success").args([
        "--request",
        "missing.json",
        "--output",
        "json",
        "--trace-file",
        "should-not-exist.jsonl",
    ]));
    let value = result(&output, 2, "failed");
    assert!(value["provider"].is_null());
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(!fixture.was_spawned());
    assert!(!fixture.root.path().join("should-not-exist.jsonl").exists());
}

#[test]
fn execution_flags_cannot_override_a_request_document() {
    let fixture = Fixture::new("codex");
    let path = fixture.request(&json!({"schema_version":1,"provider":"codex","prompt":"task"}));
    for extra in [
        vec!["--provider", "codex"],
        vec!["--prompt", "override"],
        vec!["--stdin"],
        vec!["--model", "other"],
        vec!["--effort", "high"],
        vec!["--cwd", "project"],
        vec!["--timeout", "1s"],
        vec!["--schema", "answer.json"],
        vec!["--resume", "known-session"],
        vec!["--ephemeral"],
        vec!["--", "--custom-native"],
    ] {
        let output = run(fixture
            .command("success")
            .arg("--request")
            .arg(&path)
            .args(["--output", "json"])
            .args(&extra));
        assert_eq!(
            output.status.code(),
            Some(2),
            "conflicting flags accepted: {extra:?}"
        );
        assert!(
            output.stdout.is_empty(),
            "clap errors must stay on stderr: {extra:?}"
        );
        assert!(!output.stderr.is_empty());
        assert!(!fixture.was_spawned());
    }
}

#[test]
fn accepted_request_failures_retain_the_selected_provider() {
    for provider in ["claude", "codex"] {
        for mode in ["native-conflict", "missing-cwd", "provider-failure"] {
            let fixture = Fixture::new(provider);
            let mut request = json!({"schema_version":1,"provider":provider,"prompt":"task"});
            match mode {
                "native-conflict" => {
                    request["native_args"] = if provider == "claude" {
                        json!(["--output-format", "json"])
                    } else {
                        json!(["--json"])
                    }
                }
                "missing-cwd" => request["cwd"] = json!("missing-project"),
                _ => (),
            }
            let path = fixture.request(&request);
            let provider_failed = mode == "provider-failure";
            let output = run(fixture
                .command(if provider_failed {
                    "nonzero"
                } else {
                    "success"
                })
                .arg("--request")
                .arg(path)
                .args(["--output", "json"]));
            let value = result(&output, if provider_failed { 1 } else { 2 }, "failed");
            assert_eq!(value["provider"], provider);
            assert_eq!(
                value["error"]["kind"],
                if provider_failed {
                    "provider"
                } else {
                    "invalid_input"
                }
            );
            assert_eq!(fixture.was_spawned(), provider_failed);
            if provider_failed {
                assert_eq!(value["error"]["provider_exit_code"], 7);
            }
        }
    }
}
