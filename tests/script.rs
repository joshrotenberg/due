//! Supported JavaScript command exercised as an installed, self-contained CLI.
#![cfg(all(feature = "harness", unix))]

use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    io::Write,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Fixture {
    directory: TempDir,
    script: PathBuf,
    evidence: PathBuf,
}

impl Fixture {
    fn new(source: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("workflow.js");
        let evidence = directory.path().join("evidence");
        fs::write(&script, source).unwrap();
        Self {
            directory,
            script,
            evidence,
        }
    }

    fn command(&self) -> Command {
        self.command_with_timeout("10s")
    }

    fn command_with_timeout(&self, timeout: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .arg("script")
            .arg(&self.script)
            .arg("--evidence")
            .arg(&self.evidence)
            .args(["--timeout", timeout])
            .current_dir(self.directory.path());
        command
    }

    fn provider(&self, provider: &str) -> Value {
        let native = self.directory.path().join("native");
        fs::write(&native, include_bytes!("fixtures/session-agent.sh")).unwrap();
        fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
        let workspace = self.directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(
            workspace.join("profile.json"),
            serde_json::to_vec(&json!({
                "schema_version":1,"provider":provider,"model":"fixture-profile","ephemeral":true,
            }))
            .unwrap(),
        )
        .unwrap();
        json!({"request":"profile.json","binary":native,"timeout":"5s"})
    }

    fn native_command(&self, args: &Value) -> Command {
        let mut command = self.command();
        command
            .arg("--cwd")
            .arg(self.directory.path().join("workspace"))
            .args(["--args", &args.to_string()])
            .env("DUE_SESSION_FIXTURE_DIR", self.directory.path())
            .env("DUE_SESSION_EARLY", "1");
        command
    }

    fn settled(&self, success: bool) {
        let receipt = read_json(self.evidence.join("receipt.json"));
        assert_eq!(receipt["settled"], true, "{receipt}");
        assert_eq!(receipt["exit_code"], 0, "{receipt}");
        assert_eq!(receipt["forced_kill"], false, "{receipt}");
        assert!(receipt["transport_error"].is_null(), "{receipt}");
        let workflow = read_json(self.evidence.join("workflow.json"));
        assert_eq!(
            workflow["status"],
            if success { "succeeded" } else { "failed" }
        );
    }
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn report(output: &Output, code: i32) -> Value {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(code), "{stderr}");
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid stdout JSON: {error}; {stderr}"));
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["cli_version"], env!("CARGO_PKG_VERSION"));
    assert!(report["duration_ms"].is_u64());
    assert_eq!(
        report["status"],
        if code == 0 { "succeeded" } else { "failed" }
    );
    if code == 0 {
        assert!(report["error"].is_null(), "{report}");
    } else {
        assert!(!report["error"].as_str().unwrap().is_empty(), "{report}");
        assert!(
            report["result"].is_null(),
            "failure exposed an accepted result: {report}"
        );
    }
    report
}

#[test]
fn relocated_binary_runs_javascript_and_returns_json_without_path_or_assets() {
    let fixture = Fixture::new(
        "Due.log('from the workflow'); Due.run(async scope => { await scope.sleep(1); return Due.args; });",
    );
    let binary = fixture.directory.path().join("relocated-due");
    fs::copy(env!("CARGO_BIN_EXE_due"), &binary).unwrap();
    let input = json!({"empty":{},"array":[],"null":null,"nested":[true,42,"text"]});
    let output = Command::new(binary)
        .arg("script")
        .arg(&fixture.script)
        .arg("--evidence")
        .arg(&fixture.evidence)
        .args(["--args", &input.to_string()])
        .current_dir(fixture.directory.path())
        .env_clear()
        .env("PATH", "")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let result = report(&output, 0);
    assert_eq!(result["result"], input);
    assert!(String::from_utf8_lossy(&output.stderr).contains("from the workflow"));
    for (field, file) in [
        ("stdout", "stdout.bin"),
        ("process", "receipt.json"),
        ("commands", "exec.jsonl"),
        ("workflow", "workflow.json"),
    ] {
        assert_eq!(
            Path::new(result["evidence"][field].as_str().unwrap()),
            fixture.evidence.join(file)
        );
        assert!(fixture.evidence.join(file).is_file());
    }
    fixture.settled(true);
}

#[test]
fn argument_files_and_stdin_preserve_json_and_paths_use_the_invocation_directory() {
    for stdin in [false, true] {
        let fixture = Fixture::new(
            "Due.run(async scope => ({args:Due.args,cwd:(await scope.exec({program:'/bin/pwd'})).stdout.trim()}));",
        );
        let workspace = fixture.directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let input = json!({"value":"input data","items":[null,{}]});
        fs::write(
            fixture.directory.path().join("args.json"),
            input.to_string(),
        )
        .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_due"))
            .args([
                "script",
                "workflow.js",
                "--cwd",
                "workspace",
                "--evidence",
                "evidence",
                "--args-file",
                if stdin { "-" } else { "args.json" },
            ])
            .current_dir(fixture.directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.to_string().as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        let output = child.wait_with_output().unwrap();
        let result = report(&output, 0);
        assert_eq!(result["result"]["args"], input);
        assert_eq!(
            result["result"]["cwd"],
            workspace.canonicalize().unwrap().to_string_lossy().as_ref()
        );
        fixture.settled(true);
    }
}

#[test]
fn both_native_adapters_keep_one_session_with_items_permissions_interrupt_and_reuse() {
    let source = r#"
        Due.run(async scope => {
          let items=0,decisions=0,opaque=false; const sessions=new Set();
          const spec={program:'python3',args:['-c','print("independent check")']};
          const worker=await scope.open(Due.args.profile,{
            onItem(item){items++;sessions.add(item.session_id);opaque ||= JSON.stringify(item).includes('"future":42');},
            async onInteraction(item,signal){decisions++;await scope.exec(spec,signal);
              return Due.args.provider==='claude'?{behavior:'deny',message:'fixture'}:{decision:'decline'};}
          });
          if(worker.opened.resolved_options.model!=='fixture-profile')throw Error('profile not loaded');
          const first=await worker.turn('first');
          const permission=await worker.turn('ask');
          const held=await worker.start('hold'); await worker.interrupt(held); await worker.wait(held);
          const next=await worker.turn('next');
          const check=await scope.exec(spec);
          return {first:first.status,permission:permission.text,next:next.status,check:check.stdout,
            items,decisions,opaque,sessions:sessions.size};
        });
    "#;
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(source);
        let profile = fixture.provider(provider);
        let output = fixture
            .native_command(&json!({"provider":provider,"profile":profile}))
            .output()
            .unwrap();
        let result = report(&output, 0);
        let value = &result["result"];
        assert_eq!(value["first"], "succeeded");
        assert_eq!(value["permission"], "permission denied");
        assert_eq!(value["next"], "succeeded");
        assert_eq!(value["check"], "independent check\n");
        assert_eq!(value["sessions"], 1);
        assert_eq!(value["decisions"], 1);
        assert_eq!(value["opaque"], true);
        assert!(value["items"].as_u64().unwrap() > 10);
        assert!(fixture.directory.path().join("exited").exists());
        let raw = fs::read_to_string(fixture.evidence.join("stdout.bin")).unwrap();
        assert!(raw.contains("\"future\":42"));
        fixture.settled(true);
    }
}

#[test]
fn interrupt_completed_unconsumed_turn_resolves_without_native_interrupt_and_wait_keeps_result() {
    // The terminal barrier is the turn_finished item seen by onItem; the client stores the
    // result before delivering that item, so no elapsed time is involved.
    let source = r#"
        Due.run(async scope => {
          let finish; const finished=new Promise(resolve=>{finish=resolve;});
          const worker=await scope.open(Due.args.profile,{
            onItem(item){ if(item.kind==='turn_finished') finish(item.turn_id); },
            async onInteraction(){ throw Error('unexpected interaction'); }
          });
          const rejection=async call=>{ try{ await call(); return null; } catch(e){ return String(e.message); } };
          const id=await worker.start('quick');
          if(await finished!==id) throw Error('terminal item for another turn');
          const wrong=await rejection(()=>worker.interrupt('wrong-id'));
          const empty=await rejection(()=>worker.interrupt(''));
          const ack=await worker.interrupt(id);
          const again=await worker.interrupt(id);
          const result=await worker.wait(id);
          const consumed=await rejection(()=>worker.interrupt(id));
          const next=await worker.turn('next');
          return {ack,again,wrong,empty,consumed,id,first:result.status,text:result.text,next:next.status,nextText:next.text};
        });
    "#;
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(source);
        let profile = fixture.provider(provider);
        let output = fixture
            .native_command(&json!({"provider":provider,"profile":profile}))
            .output()
            .unwrap();
        let result = report(&output, 0);
        let value = &result["result"];
        let expected =
            json!({"interrupt_requested":false,"turn_id":value["id"],"already_finished":true});
        assert_eq!(value["ack"], expected, "{provider}: {value}");
        assert_eq!(value["again"], expected, "{provider}: {value}");
        for field in ["wrong", "empty", "consumed"] {
            assert!(
                value[field]
                    .as_str()
                    .is_some_and(|message| message.contains("no active turn with that id")),
                "{provider} {field}: {value}"
            );
        }
        assert_eq!(value["first"], "succeeded", "{provider}: {value}");
        assert_eq!(value["text"], "turn 1", "{provider}: {value}");
        assert_eq!(value["next"], "succeeded", "{provider}: {value}");
        assert_eq!(value["nextText"], "turn 2", "{provider}: {value}");
        let input = fs::read_to_string(fixture.directory.path().join("input.jsonl")).unwrap();
        assert!(
            !input.contains("interrupt"),
            "{provider} sent a native interruption: {input}"
        );
        fixture.settled(true);
    }
}

#[test]
fn checked_in_example_accepts_only_after_independent_verification_passes() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(include_str!("../examples/harness/supervision.js"));
        let profile = fixture.provider(provider);
        // The checker inspects actual native input, independently of worker text.
        let verify = json!({"program":"python3","args":["-c",
            "import json,os,pathlib; rows=[json.loads(s) for s in pathlib.Path(os.environ['DUE_SESSION_FIXTURE_DIR'],'input.jsonl').read_text().splitlines()]; turns=sum(r.get('type')=='user' or r.get('method')=='turn/start' for r in rows); print('observed turns:',turns); raise SystemExit(0 if turns==2 else 1)"]});
        let output = fixture
            .native_command(
                &json!({"profile":profile,"task":"first","verify":verify,"max_revisions":1}),
            )
            .output()
            .unwrap();
        let result = report(&output, 0);
        assert_eq!(result["result"]["accepted"], true);
        let attempts = result["result"]["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0]["accepted"], false);
        assert_eq!(attempts[0]["check"]["exit_code"], 1);
        assert_eq!(attempts[1]["accepted"], true);
        assert_eq!(attempts[1]["check"]["exit_code"], 0);
        fixture.settled(true);
    }
}

#[test]
fn invalid_results_and_caught_scope_cancellation_never_report_success() {
    for source in [
        "Due.run(async()=>({callback(){}}));",
        "Due.run(async()=>{const v={};v.self=v;return v;});",
        "Due.run(async()=>({missing:undefined}));",
        "Due.run(async()=>({value:Infinity}));",
        "Due.run(async()=>({value:9007199254740992}));",
        "Due.run(async()=>('x'.repeat(1048577)));",
        "Due.run(async scope=>{scope.cancel('caller stopped');try{scope.signal.throwIfAborted();}catch{}return{accepted:true};});",
    ] {
        let fixture = Fixture::new(source);
        let output = fixture.command().output().unwrap();
        report(&output, 1);
        fixture.settled(false);
    }
    let fixture = Fixture::new("Due.run(async()=>{});");
    assert!(report(&fixture.command().output().unwrap(), 0)["result"].is_null());
}

#[test]
fn invalid_input_and_existing_evidence_are_rejected_without_overwriting() {
    let fixture = Fixture::new("Due.run(async()=>({accepted:true}));");
    let output = fixture.command().args(["--args", "{"]).output().unwrap();
    report(&output, 2);
    assert!(!fixture.evidence.exists());
    fs::create_dir(&fixture.evidence).unwrap();
    fs::write(fixture.evidence.join("sentinel"), "keep").unwrap();
    let output = fixture.command().output().unwrap();
    let result = report(&output, 2);
    assert!(result["error"].as_str().unwrap().contains("already exists"));
    assert_eq!(
        fs::read_to_string(fixture.evidence.join("sentinel")).unwrap(),
        "keep"
    );
    assert_eq!(fs::read_dir(&fixture.evidence).unwrap().count(), 1);
}

#[test]
fn evidence_paths_are_normalized_for_success_and_early_failure() {
    for absolute in [false, true] {
        for invalid_input in [false, true] {
            let mut fixture = Fixture::new("Due.run(async()=>({accepted:true}));");
            let root = fixture.directory.path().canonicalize().unwrap();
            fs::create_dir(root.join("parent")).unwrap();
            fixture.evidence = PathBuf::from("./parent//./evidence//");
            if absolute {
                fixture.evidence = root.join(&fixture.evidence);
            }
            let mut command = fixture.command();
            if invalid_input {
                command.args(["--args", "{"]);
            }
            let result = report(
                &command.output().unwrap(),
                if invalid_input { 2 } else { 0 },
            );
            let directory = root.join("parent/evidence");
            for (field, file) in [
                ("directory", ""),
                ("stdout", "stdout.bin"),
                ("process", "receipt.json"),
                ("commands", "exec.jsonl"),
                ("workflow", "workflow.json"),
            ] {
                let path = if file.is_empty() {
                    directory.clone()
                } else {
                    directory.join(file)
                };
                // Path equality itself ignores redundant components, so compare strings.
                assert_eq!(result["evidence"][field], path.to_str().unwrap());
                assert_eq!(path.exists(), !invalid_input);
            }
        }
    }
}

#[test]
fn evidence_parent_components_follow_symlinks_in_filesystem_order() {
    for linked in [false, true] {
        let mut fixture = Fixture::new("Due.run(async()=>({accepted:true}));");
        let root = fixture.directory.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("target/child")).unwrap();
        if linked {
            std::os::unix::fs::symlink("target/child", root.join("parent")).unwrap();
        } else {
            fs::create_dir(root.join("parent")).unwrap();
        }
        fixture.evidence = PathBuf::from("./parent//./../evidence");
        let result = report(&fixture.command().output().unwrap(), 0);
        assert_eq!(
            result["evidence"]["directory"],
            root.join("parent/../evidence").to_str().unwrap()
        );
        let actual = if linked {
            root.join("target/evidence")
        } else {
            root.join("evidence")
        };
        assert_eq!(
            read_json(actual.join("workflow.json"))["status"],
            "succeeded"
        );
        if linked {
            assert!(
                !root.join("evidence").exists(),
                "collapsed .. across a symlink"
            );
        }
    }
}

#[test]
fn existing_files_and_symlinks_are_invalid_evidence_before_launch() {
    for kind in ["file", "directory-link", "dangling-link"] {
        let fixture =
            Fixture::new("Due.log('workflow executed'); Due.run(async()=>({accepted:true}));");
        let target = fixture.directory.path().join("target");
        match kind {
            "file" => fs::write(&fixture.evidence, "keep").unwrap(),
            "directory-link" => {
                fs::create_dir(&target).unwrap();
                std::os::unix::fs::symlink(&target, &fixture.evidence).unwrap();
            }
            "dangling-link" => std::os::unix::fs::symlink(&target, &fixture.evidence).unwrap(),
            _ => unreachable!(),
        }
        let output = fixture.command().output().unwrap();
        let result = report(&output, 2);
        assert!(
            result["error"].as_str().unwrap().contains("already exists"),
            "{result}"
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("workflow executed"));
        if kind == "file" {
            assert_eq!(fs::read_to_string(&fixture.evidence).unwrap(), "keep");
        } else {
            assert_eq!(fs::read_link(&fixture.evidence).unwrap(), target);
            if kind == "directory-link" {
                assert_eq!(fs::read_dir(target).unwrap().count(), 0);
            } else {
                assert!(!target.exists());
            }
        }
    }
}

#[test]
fn evidence_setup_io_errors_do_not_become_invalid_input_or_skip_parent_components() {
    for path in [
        "missing/evidence",
        "missing/../evidence",
        "file/../evidence",
        "loop/evidence",
    ] {
        let mut fixture = Fixture::new("Due.run(async()=>({accepted:true}));");
        let root = fixture.directory.path();
        fs::write(root.join("file"), "keep").unwrap();
        std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
        fixture.evidence = PathBuf::from(path);
        let result = report(&fixture.command().output().unwrap(), 1);
        let error = result["error"].as_str().unwrap();
        assert!(
            error.starts_with("create evidence directory:")
                || error.starts_with("inspect evidence path:"),
            "{result}"
        );
        assert!(!root.join("evidence").exists());
        assert!(!root.join("missing").exists());
    }
}

#[test]
fn non_utf8_evidence_path_cannot_corrupt_the_json_result() {
    let fixture = Fixture::new("Due.run(async()=>({accepted:true}));");
    let evidence = fixture
        .directory
        .path()
        .join(OsString::from_vec(vec![b'e', 0xff]));
    let output = Command::new(env!("CARGO_BIN_EXE_due"))
        .arg("script")
        .arg(&fixture.script)
        .arg("--evidence")
        .arg(&evidence)
        .current_dir(fixture.directory.path())
        .output()
        .unwrap();
    let result = report(&output, 2);
    assert!(result["evidence"].is_null());
    assert!(!evidence.exists());
}

#[test]
fn workflow_expiry_waits_for_both_native_adapters_to_finish_legitimate_shutdown() {
    for provider in ["claude", "codex"] {
        let fixture = Fixture::new(
            "Due.run(async scope=>{const worker=await scope.open(Due.args.profile);await worker.turn('first');await scope.sleep(10000);});",
        );
        let profile = fixture.provider(provider);
        // Three seconds is within both native wrappers' five-second EOF grace.
        let mut native = fs::OpenOptions::new()
            .append(true)
            .open(fixture.directory.path().join("native"))
            .unwrap();
        native.write_all(b"\n/bin/sleep 3\n").unwrap();
        drop(native);
        let output = fixture
            .command_with_timeout("1s")
            .arg("--cwd")
            .arg(fixture.directory.path().join("workspace"))
            .args(["--args", &json!({"profile":profile}).to_string()])
            .env("DUE_SESSION_FIXTURE_DIR", fixture.directory.path())
            .output()
            .unwrap();
        let result = report(&output, 1);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("workflow deadline")
        );
        assert!(
            fixture.directory.path().join("exited").exists(),
            "{provider} native was still alive when script returned: {result}"
        );
        fixture.settled(false);
    }
}

#[test]
fn sigterm_cancels_the_owned_verifier_before_the_failed_result_returns() {
    signal_cancels_the_owned_verifier(libc::SIGTERM, "SIGTERM");
}

#[test]
fn sigint_cancels_the_owned_verifier_without_a_duplicate_cleanup_failure() {
    signal_cancels_the_owned_verifier(libc::SIGINT, "SIGINT");
}

#[test]
fn sigint_with_clean_settlement_reports_only_the_signal() {
    let fixture = Fixture::new(
        "Due.run(async scope=>{try{await scope.sleep(30000);}catch{}return{accepted:true};});",
    );
    let mut child = fixture
        .command()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // This file is created after the bridge installs its listeners and launches
    // Due, before evaluation. No verifier is needed for the readiness barrier.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.evidence.join("exec.jsonl").is_file() {
        if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "script did not become ready: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("script did not settle after SIGINT");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let result = report(&child.wait_with_output().unwrap(), 1);
    assert_eq!(result["error"], "runner received SIGINT");
    assert_signal_receipts(&fixture, &result, "SIGINT");
}

fn signal_cancels_the_owned_verifier(signal: i32, name: &str) {
    let fixture = Fixture::new(
        "Due.run(async scope=>{try{await scope.exec(Due.args.checker);}catch{}return{accepted:true};});",
    );
    let checker = fixture.directory.path().join("checker.sh");
    fs::write(&checker, "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$SCRIPT_FIXTURE_DIR/checker.pid\"\nexec /bin/sleep 30\n").unwrap();
    fs::set_permissions(&checker, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = fixture
        .command()
        .args([
            "--args",
            &json!({"checker":{"program":checker}}).to_string(),
        ])
        .env("SCRIPT_FIXTURE_DIR", fixture.directory.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid_path = fixture.directory.path().join("checker.pid");
    let checker_pid: i32 = loop {
        if let Ok(pid) = fs::read_to_string(&pid_path)
            && let Ok(pid) = pid.trim().parse()
        {
            break pid;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "checker did not start: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("script did not settle after {name}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let result = report(&output, 1);
    let primary = format!("runner received {name}");
    let error = result["error"].as_str().unwrap();
    assert!(error.starts_with(&primary), "{result}");
    assert_eq!(error.matches(&primary).count(), 1, "{result}");
    // Abandoning an unconsumed command is a real lifecycle failure, unlike
    // repeating the signal as if it were another cleanup failure.
    assert!(
        error.contains("workflow abandoned unconsumed exec operations"),
        "{result}"
    );
    assert_eq!(
        unsafe { libc::kill(checker_pid, 0) },
        -1,
        "verifier survived"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    let records: Vec<Value> = fs::read_to_string(fixture.evidence.join("exec.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let finished = records
        .iter()
        .find(|entry| entry["kind"] == "exec_finished")
        .unwrap();
    assert_eq!(finished["result"]["status"], "cancelled");
    assert!(finished["cleanup_error"].is_null());
    assert_signal_receipts(&fixture, &result, name);
}

fn assert_signal_receipts(fixture: &Fixture, result: &Value, name: &str) {
    // A caught host signal is still a transport failure, but the child settles.
    let receipt = read_json(fixture.evidence.join("receipt.json"));
    assert_eq!(receipt["settled"], true);
    assert_eq!(receipt["forced_kill"], false);
    assert_eq!(receipt["exit_code"], 0);
    assert_eq!(receipt["rpc_shutdown_confirmed"], true);
    assert!(receipt["cleanup_error"].is_null(), "{receipt}");
    assert_eq!(
        receipt["transport_error"],
        format!("runner received {name}")
    );
    let workflow = read_json(fixture.evidence.join("workflow.json"));
    assert_eq!(workflow["error"], result["error"]);
    assert_eq!(workflow["status"], "failed");
    assert!(workflow["result"].is_null());
}
