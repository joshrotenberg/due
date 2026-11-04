//! Caller-facing results must retain JSON fidelity and wait for owned cleanup.
#![cfg(all(unix, feature = "js"))]

use due_harness::{MAX_DOCUMENT_BYTES, Options, Outcome, run_js, run_with};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new(exit: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("exit-code"), format!("{exit}\n")).unwrap();
        Self { dir }
    }

    fn options(&self, source: &str) -> Options {
        Options {
            due: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/results-peer.sh"),
            cwd: self.dir.path().to_owned(),
            evidence: self.dir.path().join("evidence"),
            source: source.to_owned(),
            args: r#"{"flag":true,"empty":[],"missing":null}"#.into(),
            timeout: Duration::from_secs(3),
        }
    }

    fn run(&self, source: &str) -> Outcome {
        let outcome = run_js(self.options(source)).unwrap();
        let process: Value = serde_json::from_slice(
            &fs::read(self.dir.path().join("evidence/receipt.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(process["settled"], true);
        assert_eq!(process["forced_kill"], false);
        let workflow: Value = serde_json::from_slice(
            &fs::read(self.dir.path().join("evidence/workflow.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(workflow["schema_version"], 1);
        assert_eq!(workflow["result"], outcome.result);
        assert_eq!(workflow["error"], json!(outcome.error));
        outcome
    }
}

#[test]
fn args_and_awaited_return_retain_json_shapes() {
    let fixture = Fixture::new(0);
    let outcome = fixture.run(
        "Due.run(async scope => { await scope.sleep(1); return {input:Due.args, text:'é𝄞', zero:0}; });",
    );
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    assert_eq!(
        outcome.result,
        json!({"input":{"flag":true,"empty":[],"missing":null},"text":"é𝄞","zero":0})
    );
}

#[test]
fn omitted_return_is_null_but_nested_undefined_is_not_silently_removed() {
    let outcome = Fixture::new(0).run("Due.run(async () => {});");
    assert!(outcome.error.is_none());
    assert!(outcome.result.is_null());
    let outcome = Fixture::new(0).run("Due.run(async () => ({missing:undefined}));");
    assert!(outcome.error.unwrap().contains("unsupported undefined"));
    assert!(outcome.result.is_null());
}

#[test]
fn unsupported_result_values_fail_without_lossy_json_coercion() {
    for (expression, diagnostic) in [
        ("NaN", "non-finite"),
        ("Infinity", "non-finite"),
        ("9007199254740992", "unsafe integer"),
        ("-0", "negative zero"),
        ("1n", "unsupported bigint"),
        ("(() => {})", "unsupported function"),
        ("Symbol('x')", "unsupported symbol"),
        ("new Date()", "not a plain object"),
        ("new Array(2)", "sparse array"),
        ("({get x(){return 1;}})", "accessor property"),
        ("({toJSON(){return 1;}})", "unsupported function"),
        ("(()=>{const a={};a.self=a;return a;})()", "cycle"),
    ] {
        let outcome = Fixture::new(0).run(&format!("Due.run(async () => {expression});"));
        let error = outcome
            .error
            .unwrap_or_else(|| panic!("accepted {expression}"));
        assert!(error.contains(diagnostic), "{expression}: {error}");
        assert!(outcome.result.is_null());
    }
}

#[test]
fn result_limit_counts_encoded_utf8_bytes() {
    let accepted = Fixture::new(0).run(&format!(
        "Due.run(async () => 'x'.repeat({}));",
        MAX_DOCUMENT_BYTES - 2
    ));
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert_eq!(
        accepted.result.as_str().unwrap().len(),
        MAX_DOCUMENT_BYTES - 2
    );
    let rejected = Fixture::new(0).run("Due.run(async () => 'é'.repeat(524288));");
    assert!(rejected.error.unwrap().contains("result exceeds 1 MiB"));
    assert!(rejected.result.is_null());
}

#[test]
fn candidate_result_is_withheld_when_cleanup_or_cancellation_fails() {
    let outcome = Fixture::new(7).run("Due.run(async () => ({accepted:true}));");
    assert!(outcome.error.unwrap().contains("unsuccessfully"));
    assert!(outcome.result.is_null());
    let outcome = Fixture::new(0).run(
        "Due.run(async scope => {scope.cancel('stop'); try {scope.signal.throwIfAborted();} catch(e){}; return {accepted:true};});",
    );
    assert!(outcome.error.unwrap().contains("stop"));
    assert!(outcome.result.is_null());
}

#[test]
fn caught_transport_failure_is_not_cleanup_but_real_cleanup_errors_are_retained() {
    for exit in [0, 7] {
        let fixture = Fixture::new(exit);
        let outcome = run_with(fixture.options(""), "test", true, |host, _, _, _| {
            let error = host.send("{}\n{}").unwrap_err();
            assert_eq!(host.send("{}"), Err(error));
            Ok(json!({"accepted":true}))
        })
        .unwrap();
        let primary = "request must be one line of at most 1 MiB";
        let error = outcome.error.as_deref().unwrap();
        assert_eq!(error.matches(primary).count(), 1, "{error}");
        assert!(outcome.result.is_null());
        let process: Value = serde_json::from_slice(
            &fs::read(fixture.dir.path().join("evidence/receipt.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(process["transport_error"], primary);
        assert_eq!(process["settled"], true);
        assert_eq!(process["forced_kill"], false);
        assert_eq!(process["exit_code"], exit);
        assert_eq!(process["rpc_shutdown_confirmed"], exit == 0);
        if exit == 0 {
            assert_eq!(error, primary);
            assert!(process["cleanup_error"].is_null());
        } else {
            let cleanup = process["cleanup_error"].as_str().unwrap();
            assert!(cleanup.contains("Due child exited unsuccessfully"));
            assert_eq!(error, format!("{primary}; cleanup: {cleanup}"));
        }
        let workflow: Value = serde_json::from_slice(
            &fs::read(fixture.dir.path().join("evidence/workflow.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(workflow["status"], "failed");
        assert_eq!(workflow["error"], error);
        assert!(workflow["result"].is_null());
    }
}

#[test]
fn transport_failure_first_observed_during_settlement_still_withholds_result() {
    let fixture = Fixture::new(0);
    let mut options = fixture.options("");
    options.due = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bridge-peer.sh");
    fs::write(
        fixture.dir.path().join("body.sh"),
        "while IFS= read -r line; do :; done; printf '\\377\\n'\n",
    )
    .unwrap();
    let outcome = run_with(options, "test", false, |_, _, _, _| {
        Ok(json!({"accepted":true}))
    })
    .unwrap();
    assert_eq!(
        outcome.error.as_deref(),
        Some("RPC stdout contains malformed UTF-8")
    );
    assert!(outcome.result.is_null());
}

#[test]
fn invalid_inputs_reject_before_creating_evidence_or_spawning_due() {
    for scenario in ["source", "args", "json", "zero", "overflow"] {
        let fixture = Fixture::new(0);
        let mut options = fixture.options("Due.run(async () => {});");
        match scenario {
            "source" => options.source = " ".repeat(MAX_DOCUMENT_BYTES + 1),
            "args" => options.args = " ".repeat(MAX_DOCUMENT_BYTES + 1),
            "json" => options.args = "{".into(),
            "zero" => options.timeout = Duration::ZERO,
            "overflow" => options.timeout = Duration::MAX,
            _ => unreachable!(),
        }
        assert!(run_js(options).is_err(), "{scenario}");
        assert!(!fixture.dir.path().join("evidence").exists(), "{scenario}");
    }
}

#[test]
fn non_utf8_host_paths_reject_before_spawning_or_creating_evidence() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};
    for field in ["due", "cwd", "evidence"] {
        let fixture = Fixture::new(0);
        let mut options = fixture.options("Due.run(async () => {});");
        let invalid = PathBuf::from(OsString::from_vec(b"/non-utf8-\xff".to_vec()));
        match field {
            "due" => options.due = invalid,
            "cwd" => options.cwd = invalid,
            "evidence" => options.evidence = invalid,
            _ => unreachable!(),
        }
        let error = run_js(options).unwrap_err();
        assert!(error.contains("UTF-8 path"), "{field}: {error}");
        assert!(!fixture.dir.path().join("evidence").exists());
    }
}

#[test]
fn evaluator_panic_joins_commands_before_dropping_their_runtime() {
    let fixture = Fixture::new(0);
    let pid_file = fixture.dir.path().join("checker.pid");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_with(fixture.options(""), "test", true, |host, _, _, _| {
            host.exec_start(
                    r#"{"program":"/bin/sh","args":["-c","echo $$ > checker.pid; exec sleep 30"],"timeout_ms":30000}"#,
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Ok(value) = fs::read_to_string(&pid_file)
                    && value.trim().parse::<i32>().is_ok()
                {
                    break;
                }
                assert!(Instant::now() < deadline, "checker did not start");
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("evaluator failed after starting checker");
        })
    }));
    assert!(caught.is_err());
    let pid: i32 = fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "checker was not reaped");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    let events: Vec<Value> = fs::read_to_string(fixture.dir.path().join("evidence/exec.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let finished = events.last().unwrap();
    assert_eq!(finished["kind"], "exec_finished");
    assert_eq!(finished["result"]["status"], "cancelled");
    assert!(finished["cleanup_error"].is_null());
    let process: Value = serde_json::from_slice(
        &fs::read(fixture.dir.path().join("evidence/receipt.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(process["settled"], true);
    assert_eq!(process["forced_kill"], false);
}
