#![cfg(all(unix, any(feature = "workflow-spike", feature = "workflow-lua-spike")))]
#[path = "support/workflow.rs"]
mod support;
use serde_json::{Value, json};
use std::fs;
use support::*;

// One subprocess fixture and one assertion path per scenario, across both VMs.
#[derive(Clone, Copy)]
enum Language {
    Rhai,
    Lua,
}
fn languages() -> Vec<Language> {
    let mut v = vec![];
    if cfg!(feature = "workflow-spike") {
        v.push(Language::Rhai);
    }
    if cfg!(feature = "workflow-lua-spike") {
        v.push(Language::Lua);
    }
    v
}
fn fixture(language: Language, rhai: &str, lua: &str) -> Fixture {
    let f = Fixture::new(rhai);
    if matches!(language, Language::Lua) {
        f.write("main.lua", lua);
        f.edit(|m| m["entry"] = json!("main.lua"));
    }
    f
}
fn outcomes(v: &Value) -> usize {
    v["steps"].as_array().unwrap().len()
}

#[test]
fn arrival_branch_replacement_and_individual_cancel_share_one_host() {
    for language in languages() {
        let f = fixture(
            language,
            r#"
            import "patterns/v1" as p;
            let slow=due::start(p::job("slow","reader","slow"));
            let fast=due::start(p::job("fast","reader","fast"));
            let first=due::wait_any([slow,fast]);
            if first.id != "fast" { throw "arrival order"; }
            let replacement=due::start(p::job("replacement","reader","replacement"));
            let replaced=due::wait(replacement);
            let stopped=due::cancel(slow);
            let again=due::wait(fast);
            let cached=due::wait_any([replacement,fast]);
            #{first:first.id,replacement:replaced.ok,stopped:stopped.ok,again:again.id,cached:cached.id}
        "#,
            r#"
            local p=require("patterns/v1")
            local slow=due.start(p.job("slow","reader","slow"))
            local fast=due.start(p.job("fast","reader","fast"))
            local first=due.wait_any({slow,fast})
            assert(first.id == "fast")
            local replacement=due.start(p.job("replacement","reader","replacement"))
            local replaced=due.wait(replacement)
            local stopped=due.cancel(slow)
            local again=due.wait(fast)
            local cached=due.wait_any({replacement,fast})
            return {first=first.id,replacement=replaced.ok,stopped=stopped.ok,again=again.id,cached=cached.id}
        "#,
        );
        let v = f.run(false, 0);
        assert_eq!(
            v["output"],
            json!({"first":"fast","replacement":true,"stopped":false,"again":"fast","cached":"fast"})
        );
        assert!(f.root.path().join("replacement-ready").exists());
        assert_eq!(outcomes(&v), 3);
        assert_eq!(v["steps"][1]["completion_seq"], 0);
        assert_eq!(v["steps"][2]["completion_seq"], 1);
        assert_eq!(v["steps"][0]["completion_seq"], 2);
        assert_eq!(v["steps"][0]["exit"]["code"], 130);
        assert_eq!(
            f.read("artifacts/steps/slow/step.json")["child_result"]["status"],
            "cancelled"
        );
    }
}

#[test]
fn concurrency_slots_and_cancelled_queued_job_do_not_poison_sibling() {
    for language in languages() {
        let f = fixture(
            language,
            r#"
            import "patterns/v1" as p;
            let a=due::start(p::job("a","reader","barrier-a"));
            let b=due::start(p::job("b","reader","barrier-b"));
            let q=due::start(p::job("q","reader","must-not-launch"));
            let cancelled=due::cancel(q);
            due::wait(a); due::cancel(a); due::wait(b);
            cancelled
        "#,
            r#"
            local p=require("patterns/v1")
            local a=due.start(p.job("a","reader","barrier-a"))
            local b=due.start(p.job("b","reader","barrier-b"))
            local q=due.start(p.job("q","reader","must-not-launch"))
            local cancelled=due.cancel(q)
            due.wait(a); due.cancel(a); due.wait(b)
            return cancelled
        "#,
        );
        f.edit(|m| m["max_concurrency"] = json!(1));
        let mut child = f.command(false).spawn().unwrap();
        wait_for(&f.root.path().join("a-ready"), &mut child);
        wait_for(
            &f.root.path().join("artifacts/steps/q/step.json"),
            &mut child,
        );
        assert_eq!(f.calls(), vec!["barrier-a"]);
        assert!(!f.root.path().join("b-ready").exists());
        f.write("a-release", "go");
        wait_for(&f.root.path().join("b-ready"), &mut child);
        f.write("b-release", "go");
        let v = envelope(finish(child), 0);
        assert_eq!(outcomes(&v), 3);
        assert_eq!(v["output"]["exit"]["started"], false);
        assert_eq!(v["steps"][0]["ok"], true);
        assert_eq!(v["steps"][1]["ok"], true);
        assert_eq!(f.calls(), vec!["barrier-a", "barrier-b"]);
    }
}

#[test]
fn cancel_active_job_joins_while_sibling_keeps_running() {
    for language in languages() {
        let f = fixture(
            language,
            r#"
            import "patterns/v1" as p;
            let a=due::start(p::job("a","reader","barrier-a"));
            let b=due::start(p::job("b","reader","barrier-b"));
            let ready=due::start(p::job("ready","reader","ordinary"));
            due::wait(ready); due::cancel(a); due::wait(b)
        "#,
            r#"
            local p=require("patterns/v1")
            local a=due.start(p.job("a","reader","barrier-a"))
            local b=due.start(p.job("b","reader","barrier-b"))
            local ready=due.start(p.job("ready","reader","ordinary"))
            due.wait(ready); due.cancel(a); return due.wait(b)
        "#,
        );
        // The third child waits for the outer readiness release, proving both
        // cancelland and sibling are at native protocol boundaries first.
        f.write("agent", &include_str!("fixtures/workflow-agent.sh").replace("    trace-flood*)", "    ordinary*)\n        while [ ! -f \"$DUE_WORKFLOW_TEST_DIR/go\" ]; do /bin/sleep 0.01; done ;;\n    trace-flood*)"));
        f.edit(|m| m["max_concurrency"] = json!(3));
        let mut child = f.command(false).spawn().unwrap();
        wait_for(&f.root.path().join("a-ready"), &mut child);
        wait_for(&f.root.path().join("b-ready"), &mut child);
        f.write("go", "go");
        wait_for(
            &f.root.path().join("artifacts/steps/a/step.json"),
            &mut child,
        );
        assert!(!f.root.path().join("artifacts/steps/b/step.json").exists());
        assert!(child.try_wait().unwrap().is_none());
        f.write("b-release", "go");
        let v = envelope(finish(child), 0);
        assert_eq!(v["output"]["ok"], true);
        assert_eq!(v["steps"][0]["exit"]["code"], 130);
        assert_eq!(v["steps"][1]["exit"]["code"], 0);
    }
}

#[test]
fn return_throw_and_vm_exhaustion_after_start_join_all_reservations() {
    for language in languages() {
        for (rhai, lua, cause) in [
            ("42", "return 42", "outstanding"),
            ("throw \"deliberate\"", "error('deliberate')", "deliberate"),
            (
                "loop {}",
                "while true do pcall(function() while true do end end) end",
                "",
            ),
        ] {
            let f = fixture(
                language,
                &format!(
                    r#"let a=due::start(#{{id:"a",profile:"reader",prompt:"hold"}}); let b=due::start(#{{id:"b",profile:"reader",prompt:"hold"}}); {rhai}"#
                ),
                &format!(
                    r#"local a=due.start({{id="a",profile="reader",prompt="hold"}}); local b=due.start({{id="b",profile="reader",prompt="hold"}}); {lua}"#
                ),
            );
            f.edit(|m| m["max_concurrency"] = json!(1));
            let v = f.run(false, 1);
            assert_eq!(outcomes(&v), 2);
            assert!(v["error"].as_str().unwrap().contains(cause), "{v}");
            assert_eq!(v["steps"][1]["exit"]["started"], false);
            assert!(f.root.path().join("artifacts/steps/a/step.json").exists());
            assert!(f.root.path().join("artifacts/steps/b/step.json").exists());
            assert_native_cleanup(&f);
        }
    }
}

#[test]
fn invalid_handle_inputs_never_forge_jobs_and_outstanding_jobs_are_joined() {
    for language in languages() {
        for (rhai, lua) in [
            ("due::wait(#{id:\"a\"})", "due.wait({id='a'})"),
            ("due::wait_any([])", "due.wait_any({})"),
            ("due::wait_any([h,h])", "due.wait_any({h,h})"),
            ("due::wait_any([h,42])", "due.wait_any({h,42})"),
            ("json::encode(h)", "json.encode(h)"),
        ] {
            let f = fixture(
                language,
                &format!(r#"let h=due::start(#{{id:"a",profile:"reader",prompt:"hold"}}); {rhai}"#),
                &format!(
                    r#"local h=due.start({{id="a",profile="reader",prompt="hold"}}); return {lua}"#
                ),
            );
            assert_eq!(outcomes(&f.run(false, 1)), 1);
        }
    }
}

#[test]
fn receipt_collision_and_log_failure_keep_all_admitted_outcomes_and_first_cause() {
    for language in languages() {
        for collision in [false, true] {
            let f = fixture(
                language,
                r#"
                import "patterns/v1" as p;
                let h=due::start(p::job("a","reader","barrier-a"));
                let b=due::start(p::job("b","reader","barrier-b"));
                due::wait(h); due::wait(b)
            "#,
                r#"
                local p=require("patterns/v1")
                local h=due.start(p.job("a","reader","barrier-a"))
                local b=due.start(p.job("b","reader","barrier-b"))
                due.wait(h); return due.wait(b)
            "#,
            );
            if !collision {
                let (entry, prefix) = match language {
                    Language::Rhai => ("main.rhai", "for n in 0..2045 {due::log(\"x\");}\n"),
                    Language::Lua => ("main.lua", "for n=1,2045 do due.log('x') end\n"),
                };
                let source = fs::read_to_string(f.root.path().join(entry)).unwrap();
                f.write(entry, &(prefix.to_string() + &source));
            }
            let mut child = f.command(false).spawn().unwrap();
            wait_for(&f.root.path().join("a-ready"), &mut child);
            wait_for(&f.root.path().join("b-ready"), &mut child);
            if collision {
                f.write("artifacts/steps/a/step.json", "sentinel");
            }
            f.write("a-release", "go");
            let v = envelope(finish(child), 1);
            assert_eq!(outcomes(&v), 2);
            assert_eq!(v["steps"][0]["ok"], true);
            assert_eq!(v["steps"][1]["exit"]["code"], 130);
            assert!(
                v["error"].as_str().unwrap().contains(if collision {
                    "step.json"
                } else {
                    "event budget"
                }),
                "{v}"
            );
            assert_eq!(f.read("artifacts/workflow.json"), v);
            assert!(f.root.path().join("artifacts/steps/b/step.json").exists());
        }
    }
}

#[test]
fn signals_stop_active_and_queued_jobs_for_each_runtime() {
    for language in languages() {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let f = fixture(
                language,
                r#"due::parallel("g",[#{id:"a",profile:"reader",prompt:"hold"},#{id:"b",profile:"reader",prompt:"never"}])"#,
                r#"return due.parallel("g",{{id="a",profile="reader",prompt="hold"},{id="b",profile="reader",prompt="never"}})"#,
            );
            f.edit(|m| m["max_concurrency"] = json!(1));
            let mut child = f.command(false).spawn().unwrap();
            wait_for(&f.root.path().join("ready"), &mut child);
            unsafe {
                assert_eq!(libc::kill(child.id() as i32, signal), 0);
            }
            let v = envelope(finish(child), 130);
            assert_eq!(
                v["error"],
                "shutdown signal received; active Due children joined"
            );
            assert_eq!(outcomes(&v), 2);
            assert_eq!(f.calls(), vec!["hold"]);
            assert_eq!(v["steps"][0]["exit"]["code"], 130);
            assert_eq!(
                f.read("artifacts/steps/a/step.json")["child_result"]["status"],
                "cancelled"
            );
            assert_eq!(v["steps"][1]["exit"]["started"], false);
            assert_native_cleanup(&f);
        }
    }
}

#[test]
fn unavailable_language_never_falls_back_or_launches() {
    if cfg!(all(
        feature = "workflow-spike",
        feature = "workflow-lua-spike"
    )) {
        return;
    }
    let f = Fixture::new("42");
    let entry = if cfg!(feature = "workflow-lua-spike") {
        "main.rhai"
    } else {
        "main.lua"
    };
    f.write(entry, "return 42");
    f.edit(|m| m["entry"] = json!(entry));
    assert!(
        f.run(true, 1)["error"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
    assert!(
        f.run(false, 1)["error"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
    assert!(f.calls().is_empty());
    assert!(!f.root.path().join("artifacts").exists());
}
