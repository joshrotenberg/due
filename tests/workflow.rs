#![cfg(all(unix, feature = "workflow-spike"))]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "support/workflow.rs"]
mod support;
use support::*;

#[test]
fn staged_parallel_synthesis_preserves_child_and_artifacts() {
    let f = Fixture::new(
        r#"
        import "patterns/v1" as p;
        print("must not be stdout"); due::log("private diagnostic");
        due::phase("review");
        let rs = due::parallel("reviews", [p::job("a","reader","first"),p::job("b","reader","second")]);
        due::phase("synthesis");
        let answer = p::answer(due::run(p::job("s","reader",json::encode(p::answers(rs)))));
        if answer.needs_more_evidence { p::answer(due::run(p::job("follow","reader","follow"))); }
        #{order:[rs[0].id,rs[1].id], answer:answer}
    "#,
    );
    let v = f.run(false, 0);
    assert_eq!(v["status"], "succeeded");
    assert_eq!(v["output"]["order"], json!(["a", "b"]));
    assert_eq!(v["steps"].as_array().unwrap().len(), 3);
    assert_eq!(f.calls().len(), 3);
    assert_eq!(v["steps"][2]["phase"], "synthesis");
    let step = f.read("artifacts/steps/a/step.json");
    let raw = f.read("artifacts/steps/a/result.json");
    assert_eq!(step["child_result"], raw);
    assert_eq!(raw["schema_version"], 1);
    assert_eq!(raw["answer_schema_validation"], "not_performed");
    assert_eq!(step["validation"]["valid"], true);
    assert_eq!(step["exit"]["code"], 0);
    assert!(
        fs::metadata(f.root.path().join("artifacts/steps/a/trace.jsonl"))
            .unwrap()
            .len()
            > 0
    );
    assert_eq!(
        fs::metadata(f.root.path().join("artifacts"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(f.root.path().join("artifacts/steps/a/result.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(f.read("artifacts/workflow.json"), v);
    assert!(!f.root.path().join("internal-option-leaked").exists());
}

#[test]
fn event_budget_failure_keeps_every_joined_sibling() {
    let f = Fixture::new(
        r#"
        import "patterns/v1" as p;
        for n in 0..2045 { due::log("x"); }
        due::parallel("reviews", [p::job("a","reader","first"),p::job("b","reader","second")]);
    "#,
    );
    let v = f.run(false, 1);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("event budget exhausted")
    );
    assert_eq!(v["steps"].as_array().unwrap().len(), 2);
    assert_eq!(f.calls().len(), 2);
    for id in ["a", "b"] {
        let step = f.read(&format!("artifacts/steps/{id}/step.json"));
        assert_eq!(
            step["child_result"],
            f.read(&format!("artifacts/steps/{id}/result.json"))
        );
    }
}

#[test]
fn collection_cause_survives_secondary_receipt_failures() {
    let f = Fixture::new(
        r#"
        import "patterns/v1" as p;
        for n in 0..2045 { due::log("x"); }
        due::parallel("reviews", [p::job("a","reader","huge-result"),p::job("b","reader","ordinary")]);
    "#,
    );
    // Serialize launches so the output fault precedes the exhausted-log fault.
    f.edit(|m| m["max_concurrency"] = json!(1));
    let v = f.run(false, 1);
    assert_eq!(v["steps"].as_array().unwrap().len(), 2);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("step a: child output exceeded 1 MiB")
    );

    let f = Fixture::new(r#"due::run(#{id:"a",profile:"reader",prompt:"huge-result-bad-exit"})"#);
    let v = f.run(false, 1);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("step a: child output exceeded 1 MiB")
    );
    assert!(
        v["steps"][0]["error"]
            .as_str()
            .unwrap()
            .contains("exit receipt:")
    );
}

#[test]
fn host_artifact_setup_failure_stops_queued_jobs() {
    let f = Fixture::new(
        r#"
        import "patterns/v1" as p;
        due::parallel("g", [p::job("a","reader","block-request"),p::job("b","reader","two"),p::job("c","reader","never")]);
    "#,
    );
    f.edit(|m| m["max_concurrency"] = json!(1));
    let v = f.run(false, 1);
    assert_eq!(f.calls(), vec!["block-request"]);
    assert_eq!(v["steps"].as_array().unwrap().len(), 3);
    assert_eq!(v["steps"][1]["exit"]["started"], false);
    assert_eq!(v["steps"][2]["exit"]["started"], false);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("step b: create exclusive file")
    );
}

#[test]
fn check_preserves_os_signal_handling_during_blocked_input() {
    use std::os::unix::{fs::OpenOptionsExt, process::ExitStatusExt};
    let f = Fixture::new("input");
    let path = f.root.path().join("input.json");
    fs::remove_file(&path).unwrap();
    let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut child = f.command(true).spawn().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    // Opening the writer succeeds only once check is blocked reading the FIFO.
    let writer = loop {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => break file,
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                assert!(child.try_wait().unwrap().is_none());
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("FIFO writer: {error}"),
        }
    };
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let output = finish(child);
    drop(writer);
    assert_eq!(output.status.signal(), Some(libc::SIGTERM));
    assert!(f.calls().is_empty());
}

#[test]
fn child_trace_cap_does_not_change_standalone_run() {
    let f = Fixture::new(r#"due::run(#{id:"flood",profile:"reader",prompt:"trace-flood"})"#);
    let v = f.run(false, 0);
    assert_eq!(v["steps"][0]["ok"], true);
    assert_eq!(
        f.read("artifacts/steps/flood/result.json")["trace"]["incomplete"],
        true
    );
    assert!(
        fs::metadata(f.root.path().join("artifacts/steps/flood/trace.jsonl"))
            .unwrap()
            .len()
            <= 1024 * 1024
    );
    assert!(!f.root.path().join("internal-option-leaked").exists());
    let output = finish(
        Command::new(env!("CARGO_BIN_EXE_due"))
            .args([
                "run",
                "--provider",
                "claude",
                "--prompt",
                "trace-flood",
                "--output",
                "json",
                "--trace-file",
            ])
            .arg(f.root.path().join("standalone.trace.jsonl"))
            .arg("--binary")
            .arg(f.root.path().join("agent"))
            .env("DUE_WORKFLOW_TEST_DIR", f.root.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(output.status.success(), "{output:?}");
    let standalone: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(standalone["trace"]["incomplete"], false);
    assert!(
        fs::metadata(f.root.path().join("standalone.trace.jsonl"))
            .unwrap()
            .len()
            > 1024 * 1024
    );
}

#[test]
fn local_module_substitution_and_check_never_evaluate_initializers() {
    let f = Fixture::new(r#"import "local/review" as p; p::review(input.task)"#);
    f.write("local.rhai", r#"
        // Discarded on both check and run; including attempted effects and loops.
        due::run(#{id:"init",profile:"reader",prompt:"must not launch"});
        loop { }
        fn review(task) {
            import "patterns/v1" as p;
            let first = p::answer(due::run(p::job("local","reader",task)));
            if first.approved { p::answer(due::run(p::job("follow","reader","follow"))) } else { first }
        }
    "#);
    f.edit(|m| m["modules"] = json!({"local/review":"local.rhai"}));
    let checked = f.run(true, 0);
    assert_eq!(checked["dynamic_branches_verified"], false);
    assert!(f.calls().is_empty());
    assert!(!f.root.path().join("artifacts").exists());
    assert_eq!(f.run(false, 0)["steps"].as_array().unwrap().len(), 2);
    assert_eq!(f.calls(), vec!["synthetic review", "follow"]);
}

#[test]
fn check_compiles_entry_effects_without_spawning_and_rejects_invalid_modules() {
    let f = Fixture::new(r#"due::run(#{id:"entry",profile:"reader",prompt:"effect"});"#);
    f.run(true, 0);
    assert!(f.calls().is_empty());
    f.write("bad.rhai", "fn broken( {");
    f.edit(|m| m["modules"] = json!({"local/bad":"bad.rhai"}));
    assert!(
        f.run(true, 1)["error"]
            .as_str()
            .unwrap()
            .contains("local/bad")
    );
    assert!(f.calls().is_empty());
}

#[test]
fn admission_is_atomic_and_ids_global_and_jobs_cannot_override_profiles() {
    for jobs in [
        r#"[p::job("a","reader","one"),p::job("a","reader","two")]"#,
        r#"[p::job("a","reader","one"),p::job("b","missing","two")]"#,
        r#"[#{id:"a",profile:"reader",prompt:"one",native_args:[]}]"#,
        r#"[p::job("a","reader","one"),p::job("b","reader","two"),p::job("c","reader","three")]"#,
    ] {
        let f = Fixture::new(&format!(
            r#"import "patterns/v1" as p; due::parallel("g",{jobs})"#
        ));
        f.edit(|m| {
            m["max_runs"] = json!(2);
        });
        f.run(false, 1);
        assert!(f.calls().is_empty());
    }
    let f = Fixture::new(
        r#"import "patterns/v1" as p; due::run(p::job("a","reader","one")); due::run(p::job("a","reader","two"))"#,
    );
    let v = f.run(false, 1);
    assert_eq!(f.calls().len(), 1);
    assert_eq!(v["steps"].as_array().unwrap().len(), 1);
}

#[test]
fn invalid_or_failed_answer_blocks_synthesis_but_outcomes_can_be_handled() {
    for prompt in ["invalid", "fail", "missing"] {
        let f = Fixture::new(&format!(
            r#"import "patterns/v1" as p; let a=p::answer(due::run(p::job("a","reader","{prompt}"))); due::run(p::job("s","reader",json::encode(a)))"#
        ));
        let v = f.run(false, 1);
        assert_eq!(f.calls().len(), 1);
        assert_eq!(v["steps"][0]["ok"], false);
        assert!(f.root.path().join("artifacts/steps/a/step.json").exists());
    }
    let f = Fixture::new(
        r#"import "patterns/v1" as p; let failed=due::run(p::job("f","reader","fail")); if !failed.ok { p::answer(due::run(p::job("fallback","reader","fallback"))) } else { failed.answer }"#,
    );
    assert_eq!(f.run(false, 0)["output"]["approved"], true);
    assert_eq!(f.calls().len(), 2);
}

#[test]
fn script_error_vm_bound_and_output_contract_keep_receipts() {
    for end in [r#"throw "deliberate";"#, "loop {}", "'x'", "0.0 / 0.0"] {
        let f = Fixture::new(&format!(
            r#"import "patterns/v1" as p; due::run(p::job("a","reader","one")); {end}"#
        ));
        let v = f.run(false, 1);
        assert_eq!(v["steps"].as_array().unwrap().len(), 1);
        assert_eq!(f.calls().len(), 1);
        assert_eq!(f.read("artifacts/workflow.json")["status"], "failed");
    }
    let f = Fixture::new("42");
    f.json("output.schema.json", json!({"type":"string"}));
    assert!(
        f.run(false, 1)["error"]
            .as_str()
            .unwrap()
            .contains("output:")
    );
}

#[test]
fn null_presence_strict_input_and_schema_rejection() {
    let f = Fixture::new(
        r#"import "patterns/v1" as p; let s=due::run(p::job("a","reader","null")); #{present:s.answer_present, answer:p::answer(s)}"#,
    );
    f.json("answer.schema.json", json!({"type":"null"}));
    let v = f.run(false, 0);
    assert_eq!(v["output"], json!({"present":true,"answer":null}));
    for schema in [
        json!({"$ref":"https://invalid.example/schema"}),
        json!({"format":"email"}),
    ] {
        let f = Fixture::new("input");
        f.json("input.schema.json", schema);
        f.run(true, 1);
        assert!(f.calls().is_empty());
    }
    let f = Fixture::new("input");
    f.json("input.json", json!({"task":42}));
    f.run(true, 1);
    assert!(f.calls().is_empty());
}

#[test]
fn reserved_names_eval_and_undeclared_files_are_unavailable() {
    for source in [
        r#"import "patterns/v1" as due; 1"#,
        r#"let json=1; json"#,
        r#"eval("1")"#,
        r#"let input=2; input"#,
    ] {
        let f = Fixture::new(source);
        f.run(true, 1);
        assert!(f.calls().is_empty());
    }
    let f = Fixture::new(r#"import "ambient" as a; a::run()"#);
    f.write(
        "ambient.rhai",
        r#"due::run(#{id:"bad",profile:"reader",prompt:"bad"}); fn run(){1}"#,
    );
    // Check cannot prove dynamic imports; runtime static resolver rejects it.
    f.run(false, 1);
    assert!(f.calls().is_empty());
    let f = Fixture::new("1");
    f.edit(|m| m["modules"] = json!({"patterns/v1":"main.rhai"}));
    f.run(true, 1);
}

#[test]
fn unix_signals_wait_for_wrapper_cleanup_and_preserve_partial_artifacts() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let f = Fixture::new(
            r#"import "patterns/v1" as p; due::parallel("g",[p::job("active","reader","hold"),p::job("queued","reader","must-not-launch")]);"#,
        );
        f.edit(|m| m["max_concurrency"] = json!(1));
        let mut child = f.command(false).spawn().unwrap();
        wait_for(&f.root.path().join("ready"), &mut child);
        unsafe {
            assert_eq!(libc::kill(child.id() as i32, signal), 0);
        }
        let v = envelope(finish(child), 130);
        assert_eq!(v["status"], "cancelled");
        assert_eq!(f.calls(), vec!["hold"]);
        assert_eq!(v["steps"].as_array().unwrap().len(), 2);
        assert_eq!(v["steps"][0]["exit"]["code"], 130);
        assert_eq!(v["steps"][1]["exit"]["started"], false);
        let pids = fs::read_to_string(f.root.path().join("pids")).unwrap();
        for pid in pids.lines() {
            let pid: i32 = pid.parse().unwrap();
            let until = Instant::now() + Duration::from_secs(3);
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
                assert!(Instant::now() < until, "native process {pid} survived");
                thread::sleep(Duration::from_millis(10));
            }
        }
        assert!(
            f.read("artifacts/steps/active/step.json")["child_result"]["status"] == "cancelled"
        );
    }
}

#[test]
fn existing_artifacts_are_never_overwritten() {
    let f = Fixture::new("42");
    fs::create_dir(f.root.path().join("artifacts")).unwrap();
    f.write("artifacts/keep", "sentinel");
    f.run(false, 1);
    assert_eq!(
        fs::read_to_string(f.root.path().join("artifacts/keep")).unwrap(),
        "sentinel"
    );
    assert!(f.calls().is_empty());
}

#[test]
fn no_blocking_sleep_permissive_json_or_indirect_eval() {
    for source in [
        "sleep(1000)",
        "Fn(\"sleep\").call(1000)",
        "parse_json(\"#{a: 1}\")",
        "Fn(\"parse_json\").call(\"#{a: 1}\")",
        "Fn(\"eval\").call(\"42\")",
    ] {
        let f = Fixture::new(source);
        f.run(false, 1);
        assert!(f.calls().is_empty());
    }
}

#[test]
fn global_run_budget_cannot_be_caught_to_launch_more_jobs() {
    let f = Fixture::new(
        r#"
        import "patterns/v1" as p;
        due::run(p::job("a","reader","one"));
        try { due::run(p::job("b","reader","two")); } catch (e) { }
        due::run(p::job("c","reader","three"));
    "#,
    );
    f.edit(|m| {
        m["max_runs"] = json!(1);
        m["max_concurrency"] = json!(1);
    });
    let v = f.run(false, 1);
    assert_eq!(f.calls(), vec!["one"]);
    assert!(v["error"].as_str().unwrap().contains("max_runs"));
}

#[test]
fn examples_work_with_bundled_or_local_composition_without_rust_changes() {
    for local in [false, true] {
        let f = Fixture::new("");
        for (name, source) in [
            (
                "main.rhai",
                include_str!("../examples/workflow/review.rhai"),
            ),
            (
                "local-review.rhai",
                include_str!("../examples/workflow/local-review.rhai"),
            ),
            (
                "answer.schema.json",
                include_str!("../examples/workflow/answer.schema.json"),
            ),
        ] {
            f.write(name, source);
        }
        f.edit(|m| m["modules"] = json!({"local/review":"local-review.rhai"}));
        if local {
            f.write(
                "main.rhai",
                &include_str!("../examples/workflow/review.rhai").replace(
                    "import \"patterns/v1\" as review;",
                    "import \"local/review\" as review;",
                ),
            );
        }
        f.run(true, 0);
        let v = f.run(false, 0);
        assert_eq!(v["steps"].as_array().unwrap().len(), 4);
        assert_eq!(f.calls().len(), 4);
        assert_eq!(
            v["steps"][1]["phase"],
            if local { "local-review" } else { "review" }
        );
    }
}

#[test]
fn snapshots_survive_original_schema_changes_and_resolve_paths_from_manifest() {
    let f = Fixture::new(
        r#"import "patterns/v1" as p; p::answer(due::run(p::job("a","reader","mutate-schema")))"#,
    );
    let before = f.read("answer.schema.json");
    f.run(true, 0);
    let v = f.run(false, 0);
    assert_eq!(v["steps"][0]["validation"]["valid"], true);
    assert_eq!(f.read("answer.schema.json"), json!(false));
    assert_eq!(f.read("artifacts/schemas/reader.json"), before);
    let request = f.read("artifacts/steps/a/request.json");
    assert_eq!(request["cwd"], json!(f.root.path().canonicalize().unwrap()));
    assert!(Path::new(request["schema"].as_str().unwrap()).is_absolute());
    assert_eq!(
        f.read("artifacts/snapshot.json")["profiles"]["reader"]["binary"],
        json!(f.root.path().join("agent").canonicalize().unwrap())
    );
}

#[test]
fn excessive_child_output_is_bounded_and_prevents_synthesis() {
    for prompt in ["huge-result", "huge-stderr"] {
        let f = Fixture::new(&format!(
            r#"import "patterns/v1" as p; let r=due::run(p::job("a","reader","{prompt}")); due::run(p::job("s","reader","must not launch")); r"#
        ));
        let v = f.run(false, 1);
        assert_eq!(f.calls(), vec![prompt]);
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("step a: child output exceeded 1 MiB")
        );
        assert_eq!(v["steps"][0]["ok"], false);
        assert!(
            v["steps"][0]["error"]
                .as_str()
                .unwrap()
                .contains("exceeded 1 MiB")
        );
        for file in ["result.json", "stderr.txt"] {
            assert!(
                fs::metadata(f.root.path().join("artifacts/steps/a").join(file))
                    .unwrap()
                    .len()
                    <= 1024 * 1024
            );
        }
        assert!(f.read("artifacts/steps/a/exit.json")["exit"]["started"] == true);
    }
}
