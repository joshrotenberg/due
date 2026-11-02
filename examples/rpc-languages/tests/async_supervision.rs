//! The same supervised scenarios run on the same Rust bridge in both languages.
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn languages() -> Vec<&'static str> {
    vec![
        #[cfg(feature = "js")]
        "js",
        #[cfg(feature = "lua")]
        "lua",
    ]
}

fn script(language: &str, case: &str) -> String {
    if case == "verification_example" {
        return match language {
            "js" => include_str!("../workflows/supervision.js"),
            "lua" => include_str!("../workflows/supervision.lua"),
            _ => panic!("unknown language {language}"),
        }
        .to_owned();
    }
    let body = match (language, case) {
        ("js", "verification") => {
            r#"
            check((await worker.turn('Implement the repair.')).status==='succeeded','worker execution');
            const before=items; const first=await scope.exec(spec('verify'));
            check(first.status==='failed' && first.exit_code===1,'independent rejection');
            check(items>before,'items flow during verification');
            check((await worker.turn(first.stdout)).status==='succeeded','revision');
            check((await scope.exec(spec('verify'))).status==='succeeded','independent acceptance');
        "#
        }
        ("lua", "verification") => {
            r#"
            check(worker:turn('Implement the repair.').status=='succeeded','worker execution')
            local before=items; local first=scope:exec(spec('verify'))
            check(first.status=='failed' and first.exit_code==1,'independent rejection')
            check(items>before,'items flow during verification')
            check(worker:turn(first.stdout).status=='succeeded','revision')
            check(scope:exec(spec('verify')).status=='succeeded','independent acceptance')
        "#
        }
        (
            "js",
            "permission"
            | "permission_retired"
            | "permission_terminal"
            | "permission_retired_error"
            | "permission_reply_error"
            | "permission_reply_error_substring"
            | "permission_reply_stale"
            | "callback_error",
        ) => {
            "check((await worker.turn('Request permission.')).status==='succeeded','permission turn');"
        }
        (
            "lua",
            "permission"
            | "permission_retired"
            | "permission_terminal"
            | "permission_retired_error"
            | "permission_reply_error"
            | "permission_reply_error_substring"
            | "permission_reply_stale"
            | "callback_error",
        ) => "check(worker:turn('Request permission.').status=='succeeded','permission turn')",
        ("js", "verification_failure" | "deadline" | "signal") => {
            "await worker.turn('Prepare work.'); await scope.exec(spec('hold')); throw Error('checker unexpectedly completed');"
        }
        ("lua", "verification_failure" | "deadline" | "signal") => {
            "worker:turn('Prepare work.'); scope:exec(spec('hold')); error('checker unexpectedly completed')"
        }
        ("js", "active_turn") => "await worker.start('Hold this turn.');",
        ("lua", "active_turn") => "worker:start('Hold this turn.')",
        ("js", "cancel_caught") => {
            "scope.cancel(new Error('caller stop')); try {scope.signal.throwIfAborted();} catch(e) {}"
        }
        ("lua", "cancel_caught") => {
            "scope:cancel('caller stop'); pcall(function() scope.signal:throwIfAborted() end)"
        }
        ("js", "orphan") => "bridge.exec_start(JSON.stringify(spec('hold')));",
        ("lua", "orphan") => "bridge.exec_start(json.encode(spec('hold')))",
        _ => panic!("unknown {language}/{case}"),
    };
    if language == "js" {
        format!(
            r#"
        const args=JSON.parse(bridge.args), testCase={case:?}; let items=0;
        function check(v,m){{if(!v)throw Error(m);}}
        function spec(mode){{return {{program:args.checker,args:[mode],timeout_ms:5000}};}}
        Due.run(async scope=>{{
          const worker=await scope.open({{provider:'codex'}},{{
            onItem(item){{if(item.kind==='native')items++;}},
            async onInteraction(item,signal){{
              const before=items;
              if(testCase==='callback_error'){{await scope.sleep(30,signal);throw Error('delayed permission failure');}}
              if(testCase==='permission_retired'||testCase==='permission_terminal'||testCase==='permission_retired_error'){{
                let cancelled=false;
                try{{await scope.exec(spec('hold'),signal);}}catch(e){{
                  check(signal.aborted && e===signal.reason,'callback cancellation'); cancelled=true;
                }}
                check(cancelled,'checker must be cooperatively cancelled');
                if(testCase==='permission_retired_error')throw Error('distinct failure after permission retirement');
                return {{decision:'decline'}};
              }}
              const result=await scope.exec(spec('permission'),signal);
              check(result.status==='succeeded','permission checker');
              check(items>=before+2,'items flow during permission');
              return {{decision:'decline'}};
            }}
          }});
          {body}
          bridge.log('BODY_OK');
        }});
        "#
        )
    } else {
        format!(
            r#"
        local args=json.decode(bridge.args); local test_case={case:?}; local items=0
        local function check(v,m) if not v then error(m) end end
        local function spec(mode) return {{program=args.checker,args=json.array({{mode}}),timeout_ms=5000}} end
        Due.run(function(scope)
          local worker=scope:open({{provider='codex'}},{{
            onItem=function(item) if item.kind=='native' then items=items+1 end end,
            onInteraction=function(item,signal)
              local before=items
              if test_case=='callback_error' then scope:sleep(30,signal); error('delayed permission failure') end
              if test_case=='permission_retired' or test_case=='permission_terminal' or test_case=='permission_retired_error' then
                local ok,e=pcall(function() scope:exec(spec('hold'),signal) end)
                check(not ok and signal.aborted and e==signal.reason,'callback cancellation')
                if test_case=='permission_retired_error' then error('distinct failure after permission retirement') end
                return {{decision='decline'}}
              end
              local result=scope:exec(spec('permission'),signal)
              check(result.status=='succeeded','permission checker')
              check(items>=before+2,'items flow during permission')
              return {{decision='decline'}}
            end
          }})
          {body}
          bridge.log('BODY_OK')
        end)
        "#
        )
    }
}

// Only signal the child held by this test, after its owned checker confirms it
// is running. A separate deadline bounds regressions in the runner's signal path.
fn terminate_during_checker(command: &mut Command, directory: &Path) -> Output {
    let mut runner = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let ready_deadline = Instant::now() + Duration::from_secs(4);
    let mut checker_ready = false;
    while Instant::now() < ready_deadline {
        if runner.try_wait().unwrap().is_some() {
            break;
        }
        if fs::read_to_string(directory.join("checker.pid"))
            .ok()
            .and_then(|pid| pid.parse::<i32>().ok())
            .is_some_and(|pid| pid > 0 && unsafe { libc::kill(pid, 0) } == 0)
        {
            checker_ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let signal_error = if checker_ready && runner.try_wait().unwrap().is_none() {
        // The owned Child is still unreaped, so its PID cannot be reused here.
        if unsafe { libc::kill(runner.id() as i32, libc::SIGTERM) } == 0 {
            None
        } else {
            Some(std::io::Error::last_os_error())
        }
    } else {
        Some(std::io::Error::other(
            "runner exited before its checker became ready",
        ))
    };
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut timed_out = false;
    while runner.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = runner.kill();
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = runner.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(checker_ready, "checker never became ready: {stderr}");
    assert!(
        signal_error.is_none(),
        "SIGTERM delivery: {signal_error:?}: {stderr}"
    );
    assert!(!timed_out, "runner did not settle after SIGTERM: {stderr}");
    output
}

fn run_case(language: &str, case: &str, success: bool) {
    let dir = tempfile::tempdir().unwrap();
    let peer = dir.path().join("peer");
    let checker = dir.path().join("checker");
    for (path, bytes) in [
        (&peer, include_bytes!("async_peer.py").as_slice()),
        (&checker, include_bytes!("async_checker.py").as_slice()),
    ] {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let workflow = dir.path().join("workflow");
    fs::write(&workflow, script(language, case)).unwrap();
    let evidence = dir.path().join("evidence");
    let arguments = if case == "verification_example" {
        serde_json::json!({
            "profile":{"provider":"codex"},
            "task":"Repair the fixture artifact and verify the result.",
            "verify":{"program":checker,"args":["verify"],"timeout_ms":5000},
            "max_revisions":1,
        })
    } else {
        serde_json::json!({"checker":checker})
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"));
    command
        .args([language])
        .arg(&workflow)
        .arg("--async")
        .arg("--due")
        .arg(&peer)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--evidence")
        .arg(&evidence)
        .args(["--args", &arguments.to_string()])
        .args([
            "--timeout-ms",
            if case == "deadline" { "500" } else { "8000" },
        ])
        .env("RPC_ASYNC_CASE", case)
        .env("RPC_ASYNC_DIR", dir.path());
    let output = if case == "signal" {
        terminate_during_checker(&mut command, dir.path())
    } else {
        command.output().unwrap()
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.success(),
        success,
        "{language}/{case}: {stderr}"
    );
    assert!(output.stdout.is_empty(), "protocol and logs must not mix");
    let workflow: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("workflow.json")).unwrap()).unwrap();
    assert_eq!(
        workflow["status"],
        if success { "succeeded" } else { "failed" },
        "{workflow}"
    );
    assert_eq!(workflow["async"], true);
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["settled"], true, "{language}/{case}: {receipt}");
    assert_eq!(
        receipt["forced_kill"], false,
        "{language}/{case}: {receipt}"
    );
    assert!(dir.path().join("peer-exited").exists());
    if matches!(case, "signal" | "permission_retired_error") {
        assert!(
            dir.path().join("checker.pid").exists(),
            "checker was not started"
        );
        let events: Vec<serde_json::Value> = fs::read_to_string(evidence.join("exec.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let finished: Vec<_> = events
            .iter()
            .filter(|event| event["kind"] == "exec_finished")
            .collect();
        assert_eq!(finished.len(), 1, "{events:?}");
        assert_eq!(finished[0]["result"]["status"], "cancelled", "{events:?}");
        assert!(finished[0]["cleanup_error"].is_null(), "{events:?}");
        assert_eq!(receipt["exit_code"], 0, "{receipt}");
    }
    if let Ok(pid) = fs::read_to_string(dir.path().join("checker.pid")) {
        let pid: i32 = pid.parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "checker survived {language}/{case}"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
    let requests: Vec<serde_json::Value> = fs::read_to_string(dir.path().join("requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "session.open")
            .count(),
        1
    );
    if matches!(
        case,
        "permission_retired" | "permission_terminal" | "permission_retired_error"
    ) {
        assert!(
            !requests.iter().any(|r| r["method"] == "interaction.reply"),
            "late reply: {requests:?}"
        );
    }
    if matches!(case, "verification" | "verification_example") {
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "turn.start")
                .count(),
            2
        );
    }
    if case == "verification_example" {
        let events: Vec<serde_json::Value> = fs::read_to_string(evidence.join("exec.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let statuses: Vec<_> = events
            .iter()
            .filter(|event| event["kind"] == "exec_finished")
            .map(|event| event["result"]["status"].as_str().unwrap())
            .collect();
        assert_eq!(statuses, ["failed", "succeeded"], "{events:?}");
        let revision = requests
            .iter()
            .filter(|r| r["method"] == "turn.start")
            .nth(1)
            .unwrap();
        assert!(
            revision["params"]["prompt"]
                .as_str()
                .unwrap()
                .contains("artifact is incorrect"),
            "independent verifier output must reach the revision: {revision}"
        );
    }
    if !success {
        let diagnostic = workflow["error"].as_str().unwrap().to_lowercase();
        let expected = match case {
            "callback_error" => &["delayed permission failure"][..],
            "permission_retired_error" => &["distinct failure after permission retirement"][..],
            "permission_reply_error" | "permission_reply_error_substring" => {
                &["explicit reply delivery failed"][..]
            }
            "cancel_caught" => &["caller stop"][..],
            "verification_failure" => &["native failed during verification"][..],
            "deadline" => &["deadline"][..],
            "signal" => &["runner received sigterm"][..],
            "active_turn" => &["active turn", "unfinished turn"][..],
            "orphan" => &["abandon", "unjoined", "unsettled", "unconsumed"][..],
            _ => panic!("unexpected failure case"),
        };
        assert!(
            expected.iter().any(|text| diagnostic.contains(text)),
            "{language}/{case}: {diagnostic}"
        );
    }
}

#[test]
fn both_languages_verify_and_revise_without_blocking_items() {
    for language in languages() {
        run_case(language, "verification", true);
    }
}

#[test]
fn both_languages_keep_streaming_while_permissions_wait_and_cancel_retired_checks() {
    for language in languages() {
        for case in [
            "permission",
            "permission_retired",
            "permission_terminal",
            "permission_reply_stale",
        ] {
            run_case(language, case, true);
        }
    }
}

#[test]
fn both_languages_preserve_failures_and_settle_owned_work() {
    for language in languages() {
        for case in [
            "callback_error",
            "permission_reply_error",
            "permission_reply_error_substring",
            "cancel_caught",
            "verification_failure",
            "deadline",
            "active_turn",
            "orphan",
        ] {
            run_case(language, case, false);
        }
    }
}

#[test]
fn both_languages_preserve_distinct_callback_failure_after_retirement() {
    for language in languages() {
        run_case(language, "permission_retired_error", false);
    }
}

#[test]
fn both_languages_settle_checker_and_rpc_after_external_sigterm() {
    for language in languages() {
        run_case(language, "signal", false);
    }
}

#[test]
fn checked_in_examples_verify_and_revise_with_caller_owned_commands() {
    for language in languages() {
        run_case(language, "verification_example", true);
    }
}
