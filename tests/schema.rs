#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    binary: PathBuf,
    provider: &'static str,
}

impl Fixture {
    fn new(provider: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("schema-provider");
        fs::write(&binary, include_bytes!("fixtures/schema-agent.sh")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            dir,
            binary,
            provider,
        }
    }

    fn schema(&self, contents: &str) -> PathBuf {
        let path = self.dir.path().join("schema.json");
        fs::write(&path, contents).unwrap();
        path
    }

    fn command(&self, mode: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .args([
                "run",
                "--provider",
                self.provider,
                "--prompt",
                "return an answer",
                "--output",
                "json",
                "--binary",
            ])
            .arg(&self.binary)
            .current_dir(self.dir.path())
            .env("DUE_SCHEMA_DIR", self.dir.path())
            .env("DUE_SCHEMA_PROVIDER", self.provider)
            .env("DUE_SCHEMA_MODE", mode)
            .env("DUE_SCHEMA_PAYLOAD", r#"{"answer":42,"ok":true}"#)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn captured(&self, name: &str) -> Vec<u8> {
        fs::read(self.dir.path().join(name)).unwrap()
    }
}

fn run(command: &mut Command) -> Output {
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("schema invocation hung");
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

fn result(output: &Output, exit: i32, status: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], status);
    value
}

#[test]
fn shared_schema_maps_for_fresh_resumed_traced_and_buffered_runs() {
    let schema =
        json!({"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"]});
    for provider in ["claude", "codex"] {
        for traced in [false, true] {
            for resumed in [false, true] {
                let fixture = Fixture::new(provider);
                let original = fixture.schema(&schema.to_string());
                let cwd = fixture.dir.path().join("other cwd");
                fs::create_dir(&cwd).unwrap();
                fs::write(cwd.join("schema.json"), b"false").unwrap();
                let mut command = fixture.command("success");
                command
                    .args(["--schema", "schema.json", "--cwd"])
                    .arg(&cwd)
                    // Change the original after launch to prove the provider sees
                    // the captured schema, rather than a mutable original path.
                    .env("DUE_SCHEMA_ORIGINAL", &original);
                if traced {
                    command.arg("--trace=json");
                }
                if resumed {
                    command.args(["--resume", "schema-session"]);
                }
                let value = result(&run(&mut command), 0, "succeeded");
                assert_eq!(value["structured_output"], json!({"answer":42,"ok":true}));
                assert_eq!(value["session_id"], "schema-session");
                assert_eq!(value["usage"]["input_tokens"], 120);
                assert_eq!(
                    serde_json::from_slice::<Value>(&fixture.captured("schema-content")).unwrap(),
                    schema
                );
                assert_eq!(fs::read_to_string(&original).unwrap(), "false\n");
                let args = String::from_utf8(fixture.captured("argv")).unwrap();
                let mapped = if provider == "claude" {
                    "--json-schema"
                } else {
                    "--output-schema"
                };
                assert!(args.split('\0').any(|arg| arg == mapped));
                if provider == "claude" {
                    assert_eq!(value["text"], "A readable answer");
                } else {
                    assert_eq!(
                        serde_json::from_str::<Value>(value["text"].as_str().unwrap()).unwrap(),
                        value["structured_output"]
                    );
                    let snapshot =
                        PathBuf::from(String::from_utf8(fixture.captured("schema-arg")).unwrap());
                    assert_ne!(snapshot, original);
                    assert!(snapshot.is_absolute());
                }
            }
        }
    }
}

#[test]
fn schema_documents_are_validated_before_provider_launch() {
    for provider in ["claude", "codex"] {
        for contents in [None, Some("{broken"), Some("42"), Some("[]"), Some("null")] {
            let fixture = Fixture::new(provider);
            let path = fixture.dir.path().join("schema.json");
            if let Some(contents) = contents {
                fixture.schema(contents);
            }
            let value = result(
                &run(fixture.command("success").arg("--schema").arg(path)),
                2,
                "failed",
            );
            assert_eq!(value["error"]["kind"], "invalid_input");
            assert!(!fixture.dir.path().join("argv").exists());
        }
    }
}

#[test]
fn a_schema_dry_run_plans_without_requiring_a_provider_result() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let schema = fixture.schema("true");
        let value = result(
            &run(fixture
                .command("success")
                .arg("--schema")
                .arg(schema)
                .args(["--dry-run", "--trace=json"])),
            0,
            "planned",
        );
        assert!(value["invocation"]["args"].is_array());
        assert!(value["structured_output"].is_null());
        assert!(!fixture.dir.path().join("argv").exists());
    }
}

#[test]
fn common_schema_conflicts_with_native_schema_controls() {
    for provider in ["claude", "codex"] {
        let cases = if provider == "claude" {
            vec![vec!["--json-schema", "{}"], vec!["--json-schema={}"]]
        } else {
            vec![
                vec!["--output-schema", "native.json"],
                vec!["--output-schema=native.json"],
                vec!["-c", "output_schema=\"native.json\""],
                vec!["--config=output_schema=\"native.json\""],
            ]
        };
        for native in cases {
            let fixture = Fixture::new(provider);
            let schema = fixture.schema("true");
            let output = run(fixture
                .command("success")
                .arg("--schema")
                .arg(schema)
                .arg("--")
                .args(native));
            result(&output, 2, "failed");
            assert!(!fixture.dir.path().join("argv").exists());
        }
    }
}

#[test]
fn native_structured_output_is_preserved_but_codex_json_requires_common_schema() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let value = result(&run(&mut fixture.command("success")), 0, "succeeded");
        if provider == "claude" {
            assert_eq!(value["structured_output"], json!({"answer":42,"ok":true}));
            assert_eq!(value["text"], "A readable answer");
        } else {
            assert!(value["structured_output"].is_null());
            assert!(serde_json::from_str::<Value>(value["text"].as_str().unwrap()).is_ok());
            let native = fixture.schema("true");
            let value = result(
                &run(fixture
                    .command("success")
                    .args(["--", "--output-schema"])
                    .arg(native)),
                0,
                "succeeded",
            );
            assert!(
                value["structured_output"].is_null(),
                "native schema passthrough must not silently change the common output contract"
            );
        }
    }
}

#[test]
fn boolean_schema_accepts_every_json_value_type_without_text_coercion() {
    for provider in ["claude", "codex"] {
        for payload in ["null", "true", "[1,\"two\"]", "\"a JSON string\""] {
            let fixture = Fixture::new(provider);
            let schema = fixture.schema("true");
            let value = result(
                &run(fixture
                    .command("success")
                    .arg("--schema")
                    .arg(schema)
                    .env("DUE_SCHEMA_PAYLOAD", payload)),
                0,
                "succeeded",
            );
            assert_eq!(
                value["structured_output"],
                serde_json::from_str::<Value>(payload).unwrap()
            );
            if provider == "claude" {
                assert_eq!(value["text"], "A readable answer");
            }
        }
    }
}

#[test]
fn requested_output_protocol_failures_preserve_metadata_and_provider_errors_win() {
    for provider in ["claude", "codex"] {
        for traced in [false, true] {
            for mode in ["missing", "malformed", "failure"] {
                let fixture = Fixture::new(provider);
                let schema = fixture.schema("true");
                let mut command = fixture.command(mode);
                command.arg("--schema").arg(schema);
                if traced {
                    command.arg("--trace=json");
                }
                let value = result(&run(&mut command), 1, "failed");
                assert_eq!(value["session_id"], "schema-session");
                if mode == "failure" {
                    assert_eq!(value["error"]["kind"], "provider");
                    assert_eq!(value["error"]["provider_exit_code"], 9);
                    assert_eq!(value["text"], "PARTIAL ANSWER");
                } else {
                    assert_eq!(value["error"]["kind"], "protocol");
                    assert_eq!(value["usage"]["input_tokens"], 120);
                    assert_eq!(value["usage"]["output_tokens"], 45);
                }
            }
        }
    }
}
