#![cfg(unix)]

use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    binary: PathBuf,
    provider: &'static str,
}

impl Fixture {
    fn new(provider: &'static str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join(format!("fake {provider}"));
        fs::write(&binary, include_bytes!("fixtures/fake-agent.sh")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            root,
            binary,
            provider,
        }
    }

    fn command(&self, mode: &str) -> Command {
        let mut cmd = self.command_without_provider(mode);
        cmd.args(["--provider", self.provider]);
        cmd
    }

    fn command_without_provider(&self, mode: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_due"));
        cmd.args(["run", "--binary"])
            .arg(&self.binary)
            .env("DUE_TEST_DIR", self.root.path())
            .env("DUE_TEST_PROVIDER", self.provider)
            .env("DUE_TEST_MODE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn argv(&self) -> Vec<String> {
        let bytes = fs::read(self.root.path().join("argv")).unwrap();
        bytes
            .strip_suffix(&[0])
            .unwrap()
            .split(|byte| *byte == 0)
            .map(|part| String::from_utf8(part.to_vec()).unwrap())
            .collect()
    }

    fn pids(&self) -> Vec<i32> {
        fs::read_to_string(self.root.path().join("pids"))
            .unwrap()
            .lines()
            .map(|line| line.parse().unwrap())
            .collect()
    }
}

fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("CLI exceeded test deadline: {}", describe(&output));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn run(command: &mut Command) -> Output {
    finish(command.spawn().unwrap())
}

fn closed_sink() -> Stdio {
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    Stdio::from(OwnedFd::from(writer))
}

fn describe(output: &Output) -> String {
    format!(
        "status={:?}, stdout={}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json(output: &Output, code: i32, status: &str) -> Value {
    assert_eq!(output.status.code(), Some(code), "{}", describe(output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not one JSON document: {error}; {}",
            describe(output)
        )
    });
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], status);
    value
}

fn has_pair(args: &[String], first: &str, second: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == first && pair[1] == second)
}

fn wait_for(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "CLI exited before fixture became ready"
        );
        assert!(Instant::now() < deadline, "fixture did not become ready");
        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_reaped(pids: &[i32]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    for &pid in pids {
        // A zombie cannot execute or retain resources other than its process
        // table entry; a process-group child may await the host's init reaper.
        while unsafe { libc::kill(pid, 0) } == 0 {
            let ps = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&ps.stdout)
                .trim_start()
                .starts_with('Z')
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child {pid} survived CLI termination"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn fresh_runs_preserve_native_argv_and_send_exact_prompt_on_stdin() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let cwd = fixture.root.path().join("project with spaces");
        fs::create_dir(&cwd).unwrap();
        let prompt = "Keep $HOME and $(touch never-run) literal;\nsecond line — 日本語";
        let native = [
            "--custom-one",
            "a b;$HOME",
            "--custom-empty",
            "",
            "--custom-two=literal",
            "--custom-three",
            "$(false)",
        ];
        let output = run(fixture
            .command("success")
            .args([
                "--prompt",
                prompt,
                "--model",
                "native-model",
                "--effort",
                "high",
                "--cwd",
            ])
            .arg(&cwd)
            .args(["--output", "json", "--"])
            .args(native));
        let value = json(&output, 0, "succeeded");
        assert!(
            value.get("run_id").is_none(),
            "untraced stdout must retain its existing schema"
        );
        assert_eq!(value["provider"], provider);
        assert_eq!(value["text"], "FINAL ANSWER");
        assert!(value["error"].is_null());
        assert!(value["duration_ms"].is_number());
        assert_eq!(value["usage"]["input_tokens"], 120);
        assert_eq!(value["usage"]["output_tokens"], 45);
        assert_eq!(
            fs::read(fixture.root.path().join("stdin")).unwrap(),
            prompt.as_bytes()
        );
        assert_eq!(
            Path::new(
                fs::read_to_string(fixture.root.path().join("cwd"))
                    .unwrap()
                    .trim()
            ),
            cwd.canonicalize().unwrap()
        );

        let args = fixture.argv();
        assert!(!args.iter().any(|arg| arg == prompt));
        assert!(
            args.windows(native.len())
                .any(|part| part.iter().map(String::as_str).eq(native))
        );
        assert!(has_pair(&args, "--model", "native-model"));
        if provider == "claude" {
            assert!(has_pair(&args, "--effort", "high"));
            assert!(has_pair(&args, "--output-format", "json"));
            assert_eq!(value["session_id"], "claude-test-session");
            assert_eq!(value["cost_usd"], 0.0123);
        } else {
            assert_eq!(args[0], "exec");
            assert!(args.iter().any(|arg| arg == "--json"));
            assert!(
                args.iter()
                    .any(|arg| arg.contains("model_reasoning_effort") && arg.contains("high"))
            );
            assert!(args.iter().any(|arg| arg == "--output-last-message"));
            assert_eq!(args.last().map(String::as_str), Some("-"));
            assert_eq!(value["session_id"], "codex-test-thread");
            assert!(value["cost_usd"].is_null());
        }
    }
}

#[test]
fn piped_prompt_and_plain_text_output_preserve_final_answer() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let prompt = "first line\nsecond line\n";
        let mut child = fixture
            .command("success")
            .args(["--stdin", "--output", "text"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(prompt.as_bytes())
            .unwrap();
        let output = finish(child);
        assert!(output.status.success(), "{}", describe(&output));
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "FINAL ANSWER\n");
        assert_eq!(
            fs::read(fixture.root.path().join("stdin")).unwrap(),
            prompt.as_bytes()
        );
    }
}

#[test]
fn resume_and_ephemeral_map_to_provider_flags() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let resume_id = if provider == "codex" {
            "codex-test-thread"
        } else {
            "known-session"
        };
        let output = run(fixture.command("success").args([
            "--prompt", "continue", "--resume", resume_id, "--output", "json",
        ]));
        json(&output, 0, "succeeded");
        let args = fixture.argv();
        if provider == "claude" {
            assert!(has_pair(&args, "--resume", "known-session"));
        } else {
            assert_eq!(&args[..2], ["exec", "resume"]);
            assert!(has_pair(&args, resume_id, "-"));
        }

        let output = run(fixture.command("success").args([
            "--prompt",
            "temporary",
            "--ephemeral",
            "--output",
            "json",
        ]));
        json(&output, 0, "succeeded");
        let expected = if provider == "claude" {
            "--no-session-persistence"
        } else {
            "--ephemeral"
        };
        assert!(fixture.argv().iter().any(|arg| arg == expected));
    }
}

#[test]
fn codex_workspace_profile_preserves_policy_for_fresh_and_explicit_resume() {
    // This checks Due's routing and result contract, not native sandbox support
    // or enforcement. The fake provider does not persist conversation history.
    let fixture = Fixture::new("codex");
    let profile =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/codex-workspace-profile.json");
    let original_profile = fs::read(&profile).unwrap();
    let cwd = fixture.root.path().join("authorized workspace");
    fs::create_dir(&cwd).unwrap();
    let expected_config = [
        "sandbox_mode=\"workspace-write\"",
        "approval_policy=\"never\"",
        "agents.enabled=false",
        "features.multi_agent_v2=false",
        "sandbox_workspace_write.network_access=false",
        "sandbox_workspace_write.writable_roots=[]",
        "sandbox_workspace_write.exclude_slash_tmp=true",
        "sandbox_workspace_write.exclude_tmpdir_env_var=true",
    ];
    let mut session_id: Option<String> = None;
    let mut run_ids = Vec::new();
    let mut schema_paths = Vec::new();
    let mut message_paths = Vec::new();
    let mut traces = Vec::new();
    for turn in ["fresh", "resumed"] {
        let resumed = session_id.is_some();
        let prompt = format!("{turn} task\nKeep $HOME and $(false) literal.\n");
        let schema_name = format!("{turn}.schema.json");
        let schema = serde_json::json!({
            "type": "object", "properties": {turn: {"type": "string"}},
            "required": [turn], "additionalProperties": false
        });
        fs::write(fixture.root.path().join(&schema_name), schema.to_string()).unwrap();
        // A decoy proves the schema resolves from the caller, not the task cwd.
        fs::write(cwd.join(&schema_name), "false").unwrap();
        let answer = serde_json::json!({turn: "fixture answer"});
        let trace = fixture.root.path().join(format!("{turn}.jsonl"));
        assert!(!trace.exists());
        let mut command = fixture.command_without_provider("structured");
        command
            .arg("--request")
            .arg(&profile)
            .args([
                "--cwd",
                "authorized workspace",
                "--stdin",
                "--output",
                "json",
                "--schema",
                &schema_name,
                "--trace=json",
                "--trace-file",
            ])
            .arg(&trace)
            .current_dir(fixture.root.path())
            .env("DUE_TEST_ANSWER", answer.to_string())
            .env("TMPDIR", fixture.root.path())
            .stdin(Stdio::piped());
        if let Some(id) = &session_id {
            command.arg("--resume").arg(id);
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(prompt.as_bytes())
            .unwrap();
        let result = json(&finish(child), 0, "succeeded");
        assert_eq!(result["provider"], "codex");
        assert_eq!(result["session_id"], "codex-test-thread");
        assert_eq!(result["structured_output_present"], true);
        assert_eq!(result["structured_output"], answer);
        assert_eq!(result["answer_schema_validation"], "not_performed");
        assert_eq!(
            serde_json::from_str::<Value>(result["text"].as_str().unwrap()).unwrap(),
            answer
        );
        let args = fixture.argv();
        if let Some(id) = &session_id {
            assert_eq!(&args[..2], ["exec", "resume"]);
            assert_eq!(&args[args.len() - 2..], [id.as_str(), "-"]);
            assert_eq!(result["session_id"].as_str(), Some(id.as_str()));
        } else {
            assert_eq!(args[0], "exec");
            assert!(!args.iter().any(|arg| arg == "resume"));
            assert_eq!(args.last().unwrap(), "-");
        }
        let configs: Vec<_> = args
            .windows(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(configs, expected_config);
        for forbidden in [
            "--sandbox",
            "--ephemeral",
            "--last",
            "--all",
            "--model",
            "--cd",
        ] {
            assert!(!args.iter().any(|arg| arg == forbidden), "{args:?}");
        }
        assert!(args.iter().any(|arg| arg == "--json"));
        for (flag, paths) in [
            ("--output-schema", &mut schema_paths),
            ("--output-last-message", &mut message_paths),
        ] {
            paths.push(args.windows(2).find(|pair| pair[0] == flag).unwrap()[1].clone());
        }
        assert_eq!(
            fs::read(fixture.root.path().join("stdin")).unwrap(),
            prompt.as_bytes()
        );
        assert_eq!(
            Path::new(
                fs::read_to_string(fixture.root.path().join("cwd"))
                    .unwrap()
                    .trim()
            ),
            cwd.canonicalize().unwrap()
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(fixture.root.path().join("schema")).unwrap())
                .unwrap(),
            schema
        );
        let trace_bytes = fs::read(&trace).unwrap();
        let events: Vec<Value> = std::str::from_utf8(&trace_bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.first().unwrap()["event"], "run_started");
        assert_eq!(events[0]["resumed"], resumed);
        assert_eq!(events[0]["ephemeral"], false);
        assert_eq!(events[0]["dry_run"], false);
        assert!(events[0].get("timeout_ms").is_none());
        assert_eq!(events.last().unwrap()["event"], "run_finished");
        assert_eq!(events.last().unwrap()["status"], "succeeded");
        assert_eq!(events.last().unwrap()["exit_code"], 0);
        assert!(
            events
                .iter()
                .all(|event| event["run_id"] == result["run_id"])
        );
        let session_started = events
            .iter()
            .position(|event| event["event"] == "session_started")
            .expect("native thread.started must emit a trace session marker");
        assert!(
            events[session_started..]
                .iter()
                .all(|event| event["session_id"] == result["session_id"])
        );
        run_ids.push(result["run_id"].as_str().unwrap().to_owned());
        traces.push((trace, trace_bytes));
        session_id = Some(result["session_id"].as_str().unwrap().to_owned());
    }
    assert_ne!(run_ids[0], run_ids[1]);
    for path in schema_paths.iter().chain(message_paths.iter()) {
        assert!(Path::new(path).starts_with(fixture.root.path()));
        assert!(
            !Path::new(path).starts_with(&cwd),
            "caller transport must be outside worker cwd"
        );
    }
    assert_ne!(schema_paths[0], schema_paths[1]);
    assert_ne!(message_paths[0], message_paths[1]);
    for (path, bytes) in traces {
        assert_eq!(
            fs::read(path).unwrap(),
            bytes,
            "follow-up must not overwrite the first trace"
        );
    }
    assert_eq!(fs::read(profile).unwrap(), original_profile);
}

#[test]
fn replacing_codex_workspace_profile_native_args_removes_all_policy_settings() {
    let fixture = Fixture::new("codex");
    let profile =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/codex-workspace-profile.json");
    // A dry run makes this pitfall visible without launching a weakened provider.
    let result = json(
        &run(fixture
            .command_without_provider("success")
            .arg("--request")
            .arg(profile)
            .args([
                "--prompt",
                "inspect",
                "--resume",
                "explicit-session",
                "--dry-run",
                "--output",
                "json",
                "--",
                "-c",
                "model_reasoning_effort=\"high\"",
            ])),
        0,
        "planned",
    );
    let args = result["invocation"]["args"].as_array().unwrap();
    let configs: Vec<_> = args
        .windows(2)
        .filter(|pair| pair[0] == "-c")
        .map(|pair| pair[1].as_str().unwrap())
        .collect();
    assert_eq!(configs, ["model_reasoning_effort=\"high\""]);
    assert!(!fixture.root.path().join("argv").exists());
}

#[test]
fn dry_run_does_not_spawn_or_expose_prompt() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let prompt = "PRIVATE PROMPT BODY";
        let output = run(fixture.command("success").args([
            "--prompt",
            prompt,
            "--dry-run",
            "--output",
            "json",
        ]));
        let value = json(&output, 0, "planned");
        assert_eq!(value["invocation"]["stdin_bytes"], prompt.len());
        assert!(value["invocation"]["args"].is_array());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(prompt));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(prompt));
        assert!(!fixture.root.path().join("argv").exists());

        let output = run(Command::new(env!("CARGO_BIN_EXE_due"))
            .args([
                "run",
                "--provider",
                provider,
                "--prompt",
                prompt,
                "--dry-run",
                "--output",
                "json",
            ])
            .env("PATH", "")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()));
        json(&output, 0, "planned");
    }
}

#[test]
fn invalid_frontend_combinations_fail_before_spawning() {
    let fixture = Fixture::new("claude");
    let cases: &[&[&str]] = &[
        &[],
        &["--prompt", "x", "--stdin"],
        &["--prompt", "x", "--resume", "id", "--ephemeral"],
        &["--prompt", "x", "--timeout", "nonsense"],
        &["--prompt", "x", "--effort", "impossible"],
        &["--prompt", "x", "--output", "yaml"],
        &["--prompt", "x", "--unrecognized-front-end-option"],
    ];
    for args in cases {
        let output = run(fixture.command("success").args(*args));
        assert_eq!(
            output.status.code(),
            Some(2),
            "args={args:?}; {}",
            describe(&output)
        );
    }
    assert!(!fixture.root.path().join("argv").exists());
}

#[test]
fn native_transport_and_duplicate_common_options_are_rejected() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let mut cases: Vec<Vec<&str>> = vec![
            vec!["--"],
            vec!["--help"],
            vec!["--version"],
            vec!["--model=other"],
        ];
        if provider == "claude" {
            cases.extend([
                vec!["--output-format=json"],
                vec!["--input-format", "stream-json"],
                vec!["--resume=id"],
                vec!["--continue"],
                vec!["--effort=high"],
                vec!["--no-session-persistence"],
            ]);
        } else {
            cases.extend([
                vec!["--json"],
                vec!["--output-last-message=somewhere"],
                vec!["-osomewhere"],
                vec!["-mother"],
                vec!["-Celsewhere"],
                vec!["--ephemeral"],
                vec!["--last"],
                vec!["-c", "model=\"other\""],
                vec!["-cmodel=\"other\""],
                vec!["-c=model=\"other\""],
                vec!["--config=model=\"other\""],
                vec!["--config", "model_reasoning_effort=\"high\""],
                vec!["-c", " \"model\" = \"other\""],
                vec!["-c", "'model_reasoning_effort' = 'high'"],
            ]);
        }
        for native in cases {
            let output = run(fixture
                .command("success")
                .args([
                    "--prompt",
                    "x",
                    "--model",
                    "common-model",
                    "--effort",
                    "low",
                    "--",
                ])
                .args(&native));
            assert_eq!(
                output.status.code(),
                Some(2),
                "provider={provider}, native={native:?}; {}",
                describe(&output)
            );
        }
        assert!(!fixture.root.path().join("argv").exists());
    }
}

#[test]
fn native_model_and_extended_effort_work_without_common_overrides() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let native = if provider == "claude" {
            vec!["--model", "native-only", "--effort", "xhigh"]
        } else {
            vec![
                "--model",
                "native-only",
                "-c",
                "model_reasoning_effort=\"xhigh\"",
            ]
        };
        let output = run(fixture
            .command("success")
            .args(["--prompt", "x", "--output", "json", "--"])
            .args(&native));
        json(&output, 0, "succeeded");
        assert!(
            fixture
                .argv()
                .windows(native.len())
                .any(|part| part.iter().map(String::as_str).eq(native.iter().copied()))
        );
    }
}

#[test]
fn claude_short_value_aliases_preserve_values_containing_reserved_letters() {
    for native in [
        vec!["-nreview"],
        vec!["-n", "review"],
        vec!["--name", "review"],
        vec!["-nprchv"],
        vec!["-n", "prchv"],
        vec!["--name", "prchv"],
        vec!["-dfoo"],
        vec!["-d", "foo"],
        vec!["-wfoo"],
        vec!["-w", "foo"],
        vec!["-dprchv"],
        vec!["-d", "prchv"],
        vec!["-wprchv"],
        vec!["-w", "prchv"],
    ] {
        for dry_run in [false, true] {
            let fixture = Fixture::new("claude");
            let mut command = fixture.command("success");
            command.args(["--prompt", "x", "--output", "json"]);
            if dry_run {
                command.arg("--dry-run");
            }
            command.arg("--").args(&native);
            let value = json(
                &run(&mut command),
                0,
                if dry_run { "planned" } else { "succeeded" },
            );
            let args: Vec<String> = if dry_run {
                assert!(!fixture.root.path().join("argv").exists());
                serde_json::from_value(value["invocation"]["args"].clone()).unwrap()
            } else {
                fixture.argv()
            };
            assert!(
                args.windows(native.len())
                    .any(|part| { part.iter().map(String::as_str).eq(native.iter().copied()) })
            );
        }
    }
}

#[test]
fn claude_short_value_aliases_do_not_consume_extra_positionals() {
    for native in [
        vec!["-nreview", "replacement"],
        vec!["-n", "review", "replacement"],
        vec!["-dfoo", "replacement"],
        vec!["-d", "foo", "replacement"],
        vec!["-wfoo", "replacement"],
        vec!["-w", "foo", "replacement"],
    ] {
        for dry_run in [false, true] {
            let fixture = Fixture::new("claude");
            let mut command = fixture.command("success");
            command.args(["--prompt", "intended prompt", "--output", "json"]);
            if dry_run {
                command.arg("--dry-run");
            }
            command.arg("--").args(&native);
            let value = json(&run(&mut command), 2, "failed");
            assert_eq!(value["error"]["kind"], "invalid_input");
            assert!(
                value["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("unexpected native positional \"replacement\"")
            );
            assert!(!fixture.root.path().join("argv").exists());
        }
    }
}

#[test]
fn claude_short_value_aliases_do_not_hide_reserved_options_or_common_conflicts() {
    for reserved in ["p", "r", "c", "h", "v"] {
        for native in [
            vec![format!("-{reserved}")],
            vec![format!("-{reserved}nreview")],
            vec!["-nreview".into(), format!("-{reserved}")],
            vec![format!("-{reserved}dfoo")],
            vec!["-dfoo".into(), format!("-{reserved}")],
            vec![format!("-{reserved}wfoo")],
            vec!["-wfoo".into(), format!("-{reserved}")],
        ] {
            let fixture = Fixture::new("claude");
            let value = json(
                &run(fixture
                    .command("success")
                    .args(["--prompt", "x", "--output", "json", "--"])
                    .args(&native)),
                2,
                "failed",
            );
            assert_eq!(value["error"]["kind"], "invalid_input");
            assert!(!fixture.root.path().join("argv").exists());
        }
    }
    for alias in ["-nreview", "-dfoo", "-wfoo"] {
        for conflict in ["--model=other", "--effort=high", "--json-schema={}"] {
            let fixture = Fixture::new("claude");
            let schema = fixture.root.path().join("schema.json");
            fs::write(&schema, "{}").unwrap();
            let value = json(
                &run(fixture
                    .command("success")
                    .args([
                        "--prompt",
                        "x",
                        "--output",
                        "json",
                        "--model",
                        "common-model",
                        "--effort",
                        "low",
                        "--schema",
                    ])
                    .arg(schema)
                    .args(["--", alias, conflict])),
                2,
                "failed",
            );
            assert_eq!(value["error"]["kind"], "invalid_input");
            assert!(!fixture.root.path().join("argv").exists());
        }
    }
}

#[test]
fn native_initial_positionals_subcommands_and_stdin_markers_are_rejected() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let replacement = if provider == "claude" {
            vec!["replacement"]
        } else {
            vec!["resume", "injected-id"]
        };
        let after_boolean = if provider == "claude" {
            vec!["--verbose", "replacement"]
        } else {
            vec!["--skip-git-repo-check", "resume", "injected-id"]
        };
        for native in [
            replacement,
            vec!["-"],
            after_boolean,
            vec!["--unknown=foo", "replacement"],
        ] {
            let output = run(fixture
                .command("success")
                .args(["--prompt", "intended prompt", "--output", "json", "--"])
                .args(&native));
            let value = json(&output, 2, "failed");
            assert_eq!(value["error"]["kind"], "invalid_input");
            assert!(
                !fixture.root.path().join("argv").exists(),
                "provider={provider}, native={native:?}"
            );
        }
    }
}

#[test]
fn native_system_prompt_values_that_look_like_flags_are_preserved() {
    for system_prompt in ["-respect input", "--help"] {
        let fixture = Fixture::new("claude");
        let output = run(fixture.command("success").args([
            "--prompt",
            "user prompt",
            "--output",
            "json",
            "--",
            "--system-prompt",
            system_prompt,
        ]));
        json(&output, 0, "succeeded");
        assert!(has_pair(&fixture.argv(), "--system-prompt", system_prompt));
        assert_eq!(
            fs::read(fixture.root.path().join("stdin")).unwrap(),
            b"user prompt"
        );
    }
}

#[test]
fn claude_verbose_json_array_uses_its_terminal_result() {
    let fixture = Fixture::new("claude");
    let output = run(fixture.command("verbose_array").args([
        "--prompt",
        "x",
        "--output",
        "json",
        "--",
        "--verbose",
    ]));
    let value = json(&output, 0, "succeeded");
    assert_eq!(value["text"], "FINAL ANSWER");
    assert_eq!(value["session_id"], "claude-test-session");
    assert_eq!(value["cost_usd"], 0.0123);
}

#[test]
fn claude_empty_result_with_structured_output_returns_actual_json_text() {
    let fixture = Fixture::new("claude");
    let output = run(fixture
        .command("structured_empty_result")
        .args(["--prompt", "x", "--output", "json"]));
    let value = json(&output, 0, "succeeded");
    let answer: Value = serde_json::from_str(value["text"].as_str().unwrap()).unwrap();
    assert_eq!(answer, serde_json::json!({"answer": 42, "ok": true}));
}

#[test]
fn classified_claude_failures_preserve_native_exit_code_and_cap_metadata() {
    for mode in ["auth", "max_turns", "max_budget"] {
        let fixture = Fixture::new("claude");
        let mut command = fixture.command(mode);
        command.args(["--prompt", "x", "--output", "json"]);
        match mode {
            "max_turns" => {
                command.args(["--", "--max-turns", "1"]);
            }
            "max_budget" => {
                command.args(["--", "--max-budget-usd", "0.01"]);
            }
            _ => (),
        }
        let output = run(&mut command);
        let value = json(&output, 1, "failed");
        assert_eq!(
            value["error"]["kind"],
            if mode == "auth" {
                "authentication"
            } else {
                mode
            }
        );
        if mode == "auth" {
            assert_eq!(value["error"]["provider_exit_code"], 3);
            assert!(
                value["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("Not authenticated")
            );
        } else {
            assert_eq!(value["error"]["provider_exit_code"], 1);
            assert_eq!(value["session_id"], "cap-session");
            assert_eq!(
                value["cost_usd"],
                if mode == "max_turns" { 0.08 } else { 0.1273986 }
            );
        }
    }
}

#[test]
fn codex_classified_failures_use_shared_error_categories() {
    for (mode, kind) in [
        ("codex_auth", "authentication"),
        ("codex_config", "configuration"),
        ("codex_session", "session_not_found"),
    ] {
        let fixture = Fixture::new("codex");
        let output = run(fixture
            .command(mode)
            .args(["--prompt", "x", "--output", "json"]));
        let value = json(&output, 1, "failed");
        assert_eq!(value["error"]["kind"], kind);
        assert_eq!(value["error"]["provider_exit_code"], 3);
    }
}

#[test]
fn runtime_failures_are_one_json_document_with_nonzero_exit() {
    for provider in ["claude", "codex"] {
        for mode in [
            "missing_terminal",
            "semantic_failure",
            "malformed",
            "nonzero",
        ] {
            let fixture = Fixture::new(provider);
            let output = run(fixture
                .command(mode)
                .args(["--prompt", "x", "--output", "json"]));
            let value = json(&output, 1, "failed");
            assert_eq!(value["provider"], provider);
            assert!(value["error"]["kind"].is_string(), "{value}");
            assert!(
                value["error"]["message"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()),
                "{value}"
            );
            if mode == "nonzero" {
                assert_eq!(value["error"]["provider_exit_code"], 7);
                assert!(
                    value["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("fixture provider failure")
                );
            }
        }
    }
}

#[test]
fn successful_provider_stderr_is_forwarded_without_contaminating_json() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture
            .command("success_stderr")
            .args(["--prompt", "x", "--output", "json"]));
        let value = json(&output, 0, "succeeded");
        assert_eq!(value["text"], "FINAL ANSWER");
        assert!(String::from_utf8_lossy(&output.stderr).contains("fixture progress diagnostic"));
    }
}

#[test]
fn closed_stderr_preserves_final_results_and_run_exit_codes() {
    for provider in ["claude", "codex"] {
        for (mode, code, status) in [
            ("success_stderr", 0, "succeeded"),
            ("nonzero_terminal", 1, "failed"),
        ] {
            let fixture = Fixture::new(provider);
            let output = run(fixture
                .command(mode)
                .args(["--prompt", "x", "--output", "json"])
                .stderr(closed_sink()));
            let value = json(&output, code, status);
            assert_eq!(value["provider"], provider);
            if code == 0 {
                assert_eq!(value["text"], "FINAL ANSWER");
            } else {
                assert_eq!(value["text"], "PARTIAL ANSWER");
                assert_eq!(value["error"]["provider_exit_code"], 9);
            }
        }
    }
}

#[test]
fn closed_stderr_preserves_intake_error_json_and_exit_code() {
    let root = tempfile::tempdir().unwrap();
    let output = run(Command::new(env!("CARGO_BIN_EXE_due"))
        .args(["run", "--request"])
        .arg(root.path().join("missing.json"))
        .args(["--output", "json"])
        .stdout(Stdio::piped())
        .stderr(closed_sink()));
    let value = json(&output, 2, "failed");
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(value["provider"].is_null());
}

#[test]
fn stdout_failure_still_exits_one_when_stderr_is_closed() {
    let root = tempfile::tempdir().unwrap();
    for stdout in [
        closed_sink(),
        // An unconnected datagram socket fails with a non-BrokenPipe error,
        // exercising the best-effort output-error diagnostic path too.
        Stdio::from(OwnedFd::from(UnixDatagram::unbound().unwrap())),
    ] {
        let output = run(Command::new(env!("CARGO_BIN_EXE_due"))
            .args(["run", "--request"])
            .arg(root.path().join("missing.json"))
            .args(["--output", "json"])
            .stdout(stdout)
            .stderr(closed_sink()));
        assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
    }
}

#[test]
fn non_utf8_invocation_paths_fail_before_planning_or_spawning() {
    for provider in ["claude", "codex"] {
        for field in ["--binary", "--cwd"] {
            let fixture = Fixture::new(provider);
            let invalid_path = fixture
                .root
                .path()
                .join(OsString::from_vec(b"invalid-\xff".to_vec()));
            for format in ["text", "json"] {
                for dry_run in [false, true] {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
                    command
                        .args([
                            "run",
                            "--provider",
                            provider,
                            "--prompt",
                            "x",
                            "--output",
                            format,
                        ])
                        .arg(field)
                        .arg(&invalid_path)
                        .env("DUE_TEST_DIR", fixture.root.path())
                        .env("DUE_TEST_PROVIDER", provider)
                        .env("DUE_TEST_MODE", "success")
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    if field == "--cwd" {
                        command.arg("--binary").arg(&fixture.binary);
                    }
                    if dry_run {
                        command.arg("--dry-run");
                    }
                    let output = run(&mut command);
                    if format == "json" {
                        let value = json(&output, 2, "failed");
                        assert_eq!(value["error"]["kind"], "invalid_input");
                        assert_eq!(value["provider"], provider);
                        assert!(value.get("invocation").is_none());
                    } else {
                        assert_eq!(output.status.code(), Some(2), "{}", describe(&output));
                        assert!(output.stdout.is_empty(), "{}", describe(&output));
                    }
                    assert!(String::from_utf8_lossy(&output.stderr).contains("valid UTF-8"));
                    assert!(!fixture.root.path().join("argv").exists());
                }
            }
        }
    }
}

#[test]
fn nonzero_terminal_output_preserves_partial_answer_and_session() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture
            .command("nonzero_terminal")
            .args(["--prompt", "x", "--output", "json"]));
        let value = json(&output, 1, "failed");
        assert_eq!(value["text"], "PARTIAL ANSWER");
        assert_eq!(value["error"]["provider_exit_code"], 9);
        assert_eq!(
            value["session_id"],
            if provider == "claude" {
                "claude-test-session"
            } else {
                "codex-test-thread"
            }
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("fixture failure diagnostic"));
        if provider == "claude" {
            assert_eq!(value["cost_usd"], 0.5);
        }
    }
}

#[test]
fn missing_codex_final_message_is_a_protocol_error() {
    for traced in [false, true] {
        let fixture = Fixture::new("codex");
        let mut command = fixture.command("missing_final");
        command.args(["--prompt", "x", "--output", "json"]);
        if traced {
            command.arg("--trace=json");
        }
        let value = json(&run(&mut command), 1, "failed");
        assert_eq!(value["error"]["kind"], "protocol");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("final-message file")
        );
        assert!(value["error"]["provider_exit_code"].is_null());
        assert_eq!(value["text"], "FINAL ANSWER");
        assert_eq!(value["session_id"], "codex-test-thread");
        assert_eq!(
            value["usage"],
            serde_json::json!({
                "input_tokens": 120,
                "cached_input_tokens": 0,
                "cache_write_input_tokens": 0,
                "output_tokens": 45,
                "reasoning_output_tokens": 0,
                "total_tokens": null,
            })
        );
    }
}

#[test]
fn codex_failed_turn_text_output_does_not_print_received_assistant_text() {
    for traced in [false, true] {
        for mode in ["missing_final", "codex_partial_exit", "codex_partial_eof"] {
            let fixture = Fixture::new("codex");
            let mut command = fixture.command(mode);
            command.args(["--prompt", "x", "--output", "text"]);
            if traced {
                command.arg("--trace=json");
            }
            let output = run(&mut command);
            assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
            assert!(output.stdout.is_empty(), "{}", describe(&output));
        }
    }
}

#[test]
fn codex_incomplete_turn_preserves_received_text_and_failure() {
    for traced in [false, true] {
        for (mode, text, kind, native_exit) in [
            (
                "codex_partial_exit",
                "SYNTHETIC PARTIAL ANSWER",
                "provider",
                Some(9),
            ),
            ("codex_no_message_exit", "", "provider", Some(9)),
            (
                "codex_partial_eof",
                "SYNTHETIC PARTIAL ANSWER",
                "protocol",
                None,
            ),
            ("codex_no_message_eof", "", "protocol", None),
            ("nonzero_terminal", "PARTIAL ANSWER", "provider", Some(9)),
        ] {
            let fixture = Fixture::new("codex");
            let mut command = fixture.command(mode);
            command.args(["--prompt", "x", "--output", "json"]);
            if traced {
                command.arg("--trace=json");
            }
            let value = json(&run(&mut command), 1, "failed");
            assert_eq!(value["provider"], "codex");
            assert_eq!(value["text"], text, "{mode}, traced={traced}");
            assert_eq!(value["session_id"], "codex-test-thread");
            assert_eq!(value["error"]["kind"], kind);
            assert_eq!(
                value["error"]["provider_exit_code"],
                serde_json::json!(native_exit)
            );
            assert!(value["usage"].is_null());
            assert!(value["cost_usd"].is_null());
            assert_eq!(value["structured_output_present"], false);
            assert_eq!(value["answer_schema_validation"], "not_requested");
            if mode == "nonzero_terminal" {
                assert_eq!(value["error"]["message"], "fixture terminal failure");
            }
        }
    }
}

#[test]
fn codex_partial_schema_answers_keep_failure_and_validation_intent() {
    for traced in [false, true] {
        for text in ["SYNTHETIC PARTIAL ANSWER", r#"{"answer":42}"#] {
            let fixture = Fixture::new("codex");
            let schema = fixture.root.path().join("schema.json");
            // Even JSON that cannot satisfy this schema is only decoded, not validated.
            fs::write(&schema, "false").unwrap();
            let mut command = fixture.command("codex_partial_exit");
            command
                .args(["--prompt", "x", "--output", "json", "--schema"])
                .arg(schema)
                .env(
                    "DUE_TEST_PARTIAL_TEXT",
                    serde_json::to_string(text).unwrap(),
                );
            if traced {
                command.arg("--trace=json");
            }
            let value = json(&run(&mut command), 1, "failed");
            assert_eq!(value["text"], text);
            assert_eq!(value["session_id"], "codex-test-thread");
            assert_eq!(value["error"]["kind"], "provider");
            assert_eq!(value["error"]["provider_exit_code"], 9);
            let decoded = serde_json::from_str::<Value>(text).ok();
            assert_eq!(value["structured_output_present"], decoded.is_some());
            assert_eq!(value["structured_output"], decoded.unwrap_or(Value::Null));
            assert_eq!(value["answer_schema_validation"], "not_performed");
        }
    }
}

#[test]
fn codex_stopped_turn_keeps_only_available_partial_data_and_reaps_processes() {
    let partial = r#"{"answer":42}"#;
    for traced in [false, true] {
        for with_message in [false, true] {
            for cancel in [false, true] {
                let fixture = Fixture::new("codex");
                let mut command = fixture.command(if with_message {
                    "codex_partial_stop"
                } else {
                    "codex_no_message_stop"
                });
                let schema = fixture.root.path().join("schema.json");
                fs::write(&schema, "false").unwrap();
                command
                    .args([
                        "--prompt",
                        "x",
                        "--output",
                        "json",
                        "--timeout",
                        if cancel { "30s" } else { "2s" },
                        "--schema",
                    ])
                    .arg(schema)
                    .env(
                        "DUE_TEST_PARTIAL_TEXT",
                        serde_json::to_string(partial).unwrap(),
                    );
                let trace = fixture.root.path().join("trace.jsonl");
                if traced {
                    command.arg("--trace-file").arg(&trace);
                }
                let mut child = command.spawn().unwrap();
                wait_for(&fixture.root.path().join("pids"), &mut child);
                let pids = fixture.pids();
                if cancel {
                    if traced {
                        // Wait for Due to observe the event after the last message,
                        // not just for the fixture to have written it to a pipe.
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !fs::read_to_string(&trace)
                            .unwrap_or_default()
                            .contains("\"activity_id\":\"partial-ready\"")
                        {
                            assert!(child.try_wait().unwrap().is_none());
                            assert!(
                                Instant::now() < deadline,
                                "partial events were not observed"
                            );
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
                }
                let output = finish(child);
                assert_reaped(&pids);
                assert_eq!(
                    fs::read_to_string(fixture.root.path().join("interrupt")).unwrap(),
                    "SIGINT\n"
                );
                let (code, status, kind) = if cancel {
                    (130, "cancelled", "cancelled")
                } else {
                    (124, "timed_out", "timeout")
                };
                let value = json(&output, code, status);
                assert_eq!(value["error"]["kind"], kind);
                assert!(value["error"]["provider_exit_code"].is_null());
                if traced {
                    assert!(
                        fs::read_to_string(&trace)
                            .unwrap()
                            .contains("\"activity_id\":\"partial-ready\""),
                        "partial events were not observed: with_message={with_message}, cancel={cancel}"
                    );
                }
                let has_partial = traced && with_message;
                assert_eq!(value["text"], if has_partial { partial } else { "" });
                assert_eq!(
                    value["session_id"],
                    if traced {
                        serde_json::json!("codex-test-thread")
                    } else {
                        Value::Null
                    }
                );
                assert!(value["usage"].is_null());
                assert!(value["cost_usd"].is_null());
                assert_eq!(value["structured_output_present"], has_partial);
                assert_eq!(
                    value["structured_output"],
                    if has_partial {
                        serde_json::json!({"answer":42})
                    } else {
                        Value::Null
                    }
                );
                assert_eq!(value["answer_schema_validation"], "not_performed");
            }
        }
    }
}

#[test]
fn malformed_claude_terminal_fields_are_protocol_errors() {
    for mode in [
        "wrong_type",
        "wrong_subtype_type",
        "wrong_error_type",
        "wrong_result_type",
        "wrong_session_type",
    ] {
        let fixture = Fixture::new("claude");
        let output = run(fixture
            .command(mode)
            .args(["--prompt", "x", "--output", "json"]));
        let value = json(&output, 1, "failed");
        assert_eq!(value["error"]["kind"], "protocol", "mode={mode}, {value}");
    }
}

#[test]
fn missing_binary_is_a_structured_runtime_error() {
    for provider in ["claude", "codex"] {
        let root = tempfile::tempdir().unwrap();
        let output = run(Command::new(env!("CARGO_BIN_EXE_due"))
            .args([
                "run",
                "--provider",
                provider,
                "--prompt",
                "x",
                "--output",
                "json",
                "--binary",
            ])
            .arg(root.path().join("does-not-exist"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()));
        let value = json(&output, 127, "failed");
        assert!(value["error"]["kind"].is_string());
    }
}

#[test]
fn binary_resolution_uses_original_directory_before_changing_child_cwd() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let cwd = fixture.root.path().join("different working directory");
        fs::create_dir(&cwd).unwrap();
        let default_binary = fixture.root.path().join(provider);
        fs::copy(&fixture.binary, &default_binary).unwrap();
        fs::set_permissions(&default_binary, fs::Permissions::from_mode(0o755)).unwrap();

        for explicit_relative in [true, false] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
            command
                .args([
                    "run",
                    "--provider",
                    provider,
                    "--prompt",
                    "x",
                    "--output",
                    "json",
                    "--cwd",
                ])
                .arg(&cwd)
                .current_dir(fixture.root.path())
                .env("DUE_TEST_DIR", fixture.root.path())
                .env("DUE_TEST_PROVIDER", provider)
                .env("DUE_TEST_MODE", "success")
                .env("PATH", fixture.root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if explicit_relative {
                command
                    .arg("--binary")
                    .arg(Path::new(".").join(fixture.binary.file_name().unwrap()));
            }
            let output = run(&mut command);
            json(&output, 0, "succeeded");
            assert_eq!(
                Path::new(
                    fs::read_to_string(fixture.root.path().join("cwd"))
                        .unwrap()
                        .trim()
                ),
                cwd.canonicalize().unwrap()
            );
        }
    }
}

#[test]
fn timeout_stops_provider_and_its_child() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let output = run(fixture.command("blocked").args([
            "--prompt",
            "x",
            "--output",
            "json",
            "--timeout",
            // Leave room for macOS process startup under parallel test load;
            // the fixture's sleep is 30s, so this still verifies real timeout.
            "2s",
        ]));
        json(&output, 124, "timed_out");
        assert_reaped(&fixture.pids());
    }
}

#[test]
fn ctrl_c_stops_provider_and_its_child() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(provider);
        let mut child = fixture
            .command("blocked")
            .args(["--prompt", "x", "--output", "json", "--timeout", "30s"])
            .spawn()
            .unwrap();
        wait_for(&fixture.root.path().join("pids"), &mut child);
        let pids = fixture.pids();
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        let output = finish(child);
        json(&output, 130, "cancelled");
        assert_reaped(&pids);
    }
}

#[test]
fn termination_and_hangup_clean_up_providers_and_return_cancellation_json() {
    for provider in ["claude", "codex"] {
        for signal in [libc::SIGTERM, libc::SIGHUP] {
            for traced in [false, true] {
                let fixture = Fixture::new(provider);
                let mut command = fixture.command("interruptible");
                command.args(["--prompt", "x", "--output", "json", "--timeout", "30s"]);
                if traced {
                    command.arg("--trace=json");
                }
                let mut child = command.spawn().unwrap();
                wait_for(&fixture.root.path().join("pids"), &mut child);
                let pids = fixture.pids();
                assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
                let output = finish(child);
                assert_reaped(&pids);
                let result = json(&output, 130, "cancelled");
                assert_eq!(result["error"]["kind"], "cancelled");
                assert_eq!(result["provider"], provider);
                assert_eq!(
                    fs::read_to_string(fixture.root.path().join("interrupt")).unwrap(),
                    "SIGINT\n",
                    "{provider}: signal {signal} did not reach native cleanup"
                );
            }
        }
    }
}

#[test]
fn cooperative_native_shutdown_keeps_the_original_stop_outcome() {
    for provider in ["claude", "codex"] {
        for traced in [false, true] {
            for cancel in [false, true] {
                let fixture = Fixture::new(provider);
                let mut command = fixture.command("interruptible");
                command.args([
                    "--prompt",
                    "x",
                    "--output",
                    "json",
                    "--timeout",
                    if cancel { "30s" } else { "2s" },
                ]);
                if traced {
                    command.arg("--trace=json");
                }
                let mut child = command.spawn().unwrap();
                wait_for(&fixture.root.path().join("pids"), &mut child);
                let pids = fixture.pids();
                if cancel {
                    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
                }
                let output = finish(child);
                let (code, status, kind) = if cancel {
                    (130, "cancelled", "cancelled")
                } else {
                    (124, "timed_out", "timeout")
                };
                let result = json(&output, code, status);
                assert_eq!(result["error"]["kind"], kind);
                assert_eq!(
                    fs::read_to_string(fixture.root.path().join("interrupt")).unwrap(),
                    "SIGINT\n",
                    "{provider}: native cleanup handler did not run"
                );
                assert_reaped(&pids);
            }
        }
    }
}
