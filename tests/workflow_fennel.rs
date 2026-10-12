#![cfg(all(unix, feature = "workflow-fennel-spike"))]
#[path = "support/workflow.rs"]
mod support;
use serde_json::{Value, json};
use std::{fs, process::Command};
use support::*;

fn fixture(source: &str, language: &str) -> Fixture {
    let f = Fixture::new("");
    let name = format!("main.{language}");
    f.write(&name, source);
    f.edit(|m| {
        m["schema_version"] = json!(2);
        m["entry"] = json!(name);
    });
    f
}
fn fennel(body: &str) -> Fixture {
    fixture(&format!("(fn [input]\n{body})"), "fnl")
}
fn error(f: &Fixture, check: bool) -> String {
    let result = f.run(check, 1);
    assert!(f.calls().is_empty(), "{result}");
    result["error"].as_str().unwrap().to_owned()
}

#[test]
fn arrival_replacement_cancellation_and_receipts_in_both_v2_languages() {
    let lua = r#"local p=require('patterns/v1')
return function(input)
 local slow=due.start(p.job('slow','reader','slow'))
 local fast=due.start(p.job('fast','reader','fast'))
 local first=due.wait_any({slow,fast})
 assert(first.id=='fast' and first.answer.approved)
 local replacement=due.start(p.job('replacement','reader','replacement'))
 local done=due.wait(replacement)
 local stopped=due.cancel(slow)
 assert(due.wait(fast).completion_seq==first.completion_seq)
 return {first=first,cancelled=stopped,replacement=done}
end"#;
    let fnl = r#"(local p (require :patterns/v1))
(fn [input]
 (local slow (due.start (p.job :slow :reader :slow)))
 (local fast (due.start (p.job :fast :reader :fast)))
 (local first (due.wait_any [slow fast]))
 (assert (and (= first.id :fast) first.answer.approved))
 (local replacement (due.start (p.job :replacement :reader :replacement)))
 (local done (due.wait replacement))
 (local stopped (due.cancel slow))
 (assert (= (. (due.wait fast) :completion_seq) first.completion_seq))
 {:first first :cancelled stopped :replacement done})"#;
    for (language, source) in [("lua", lua), ("fnl", fnl)] {
        let f = fixture(source, language);
        f.run(true, 0);
        assert!(f.calls().is_empty());
        let v = f.run(false, 0);
        assert_eq!(v["steps"].as_array().unwrap().len(), 3);
        assert_eq!(v["output"]["first"]["id"], "fast");
        assert_eq!(v["output"]["cancelled"]["exit"]["code"], 130);
        assert_eq!(v["output"]["replacement"]["ok"], true);
        assert!(f.root.path().join("replacement-ready").exists());
        for name in ["slow", "fast", "replacement"] {
            assert_eq!(
                f.read(&format!("artifacts/steps/{name}/step.json"))["id"],
                name
            );
            f.read(&format!("artifacts/steps/{name}/exit.json"));
        }
        assert_eq!(f.read("artifacts/workflow.json"), v);
        assert_native_cleanup(&f);
    }
}

#[test]
fn mixed_modules_cache_nested_imports_and_reusable_component() {
    for (language, source) in [
        (
            "lua",
            "local c=require('local/f'); return function(input) return c.answer(input) end",
        ),
        (
            "fnl",
            "(local c (require :local/l)) (fn [input] (c.answer input))",
        ),
    ] {
        let f = fixture(source, language);
        f.write(
            "functions.fnl",
            "(local c (require :local/leaf)) {:answer (fn [input] (+ 2 (c.value)))}",
        );
        f.write("lua.lua", "local c=require('local/f'); assert(c==require('local/f')); return {answer=function(input) return c.answer(input) end}");
        f.write(
            "leaf.lua",
            "local constant=40; return {value=function() return constant end}",
        );
        f.edit(|m| {
            m["modules"] =
                json!({"local/f":"functions.fnl","local/l":"lua.lua","local/leaf":"leaf.lua"})
        });
        assert_eq!(f.run(false, 0)["output"], 42);
        let snapshot = f.read("artifacts/snapshot.json");
        assert!(snapshot["fennel"]["compiled"]["functions.fnl"]["map"].is_array());
        assert!(snapshot["fennel"]["original"]["functions.fnl"].is_string());
        assert_eq!(
            snapshot["fennel"]["sha256"],
            "c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2"
        );
    }
}

#[test]
fn v1_lua_entry_with_fennel_module_snapshot_shim_and_loading_rules() {
    let f = fixture(
        r#"
        assert(setmetatable==nil and debug==nil)
        assert(getmetatable({})==nil and getmetatable('string')==nil)
        assert(getmetatable(json.array())==nil and getmetatable(json.object())==nil)
        assert(getmetatable(json.null)==nil)
        return require('local/f').answer()
    "#,
        "lua",
    );
    let source = "{:answer (fn [] (local [head & rest] [1 2 3]) {:head head :rest rest})}";
    f.write("functions.fnl", source);
    f.edit(|m| {
        m["schema_version"] = json!(1);
        m["modules"] = json!({"local/f":"functions.fnl"});
    });
    f.run(true, 0);
    assert!(!f.root.path().join("artifacts").exists());
    assert_eq!(f.run(false, 0)["output"], json!({"head":1,"rest":[2,3]}));
    let snapshot = f.read("artifacts/snapshot.json");
    assert_eq!(snapshot["fennel"]["version"], "1.6.1");
    assert_eq!(snapshot["fennel"]["original"]["functions.fnl"], source);
    assert!(
        snapshot["fennel"]["compiled"]["functions.fnl"]["lua"]
            .as_str()
            .unwrap()
            .contains("getmetatable")
    );
    assert!(snapshot["fennel"]["compiled"]["functions.fnl"]["map"].is_array());
    assert!(snapshot["fennel"]["compiled"].get("main.lua").is_none());
    assert!(f.calls().is_empty());

    // Fennel in a v1 bundle opts into sticky ordinary module failures, too.
    for (source, expected) in [
        ("(pcall (fn [] (due.log :init))) {}", "effects unavailable"),
        (
            "(due.start {:id :init :profile :reader :prompt :never}) {}",
            "effects unavailable",
        ),
        ("(error :bad-initializer)", "bad-initializer"),
        ("(require :local/f)", "cyclic module initialization"),
        ("(require :missing)", "undeclared module missing"),
        ("{:bad 42}", "function table"),
    ] {
        let f = fixture("pcall(require,'local/f'); return true", "lua");
        f.write("functions.fnl", source);
        f.edit(|m| {
            m["schema_version"] = json!(1);
            m["modules"] = json!({"local/f":"functions.fnl"});
        });
        f.run(true, 0);
        assert!(!f.root.path().join("artifacts").exists());
        let message = error(&f, false);
        assert!(message.contains(expected), "{message}");
    }
    // The rule applies to the bundle, even for a failed Lua import before the
    // Fennel module is first required.
    let f = fixture("pcall(require,'missing'); return true", "lua");
    f.write("functions.fnl", "{}");
    f.edit(|m| {
        m["schema_version"] = json!(1);
        m["modules"] = json!({"local/f":"functions.fnl"});
    });
    assert!(error(&f, false).contains("undeclared module missing"));
}

#[test]
fn loading_is_effect_disabled_and_failures_are_sticky_even_when_caught() {
    for (language, source) in [
        (
            "lua",
            "pcall(function() due.start({id='init',profile='reader',prompt='never'}) end); return function(input) return true end",
        ),
        ("fnl", "(pcall (fn [] (due.log :effect))) (fn [input] true)"),
        ("lua", "return 42"),
        ("fnl", "42"),
    ] {
        let f = fixture(source, language);
        f.run(true, 0); // check does not execute entry or require its result
        assert!(!f.root.path().join("artifacts").exists());
        assert!(!error(&f, false).is_empty());
    }
    for module in [
        "error('module failure')",
        "return 42",
        "return require('local/m')",
        "return require('missing')",
        "pcall(function() due.log('init') end); return {}",
    ] {
        let f = fixture(
            "pcall(function() require('local/m') end); return function(input) return true end",
            "lua",
        );
        f.write("module.lua", module);
        f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
        f.run(true, 0);
        error(&f, false);
    }
    let f = fennel("(require :ambient)");
    f.write("ambient.fnl", "{:get (fn [] 42)}");
    assert!(error(&f, false).contains("undeclared module ambient"));
}

#[test]
fn compile_time_surfaces_are_rejected_and_ambient_paths_are_ignored() {
    for source in [
        "(include :canary)",
        "(import-macros x :canary)",
        "(require-macros :canary)",
        "(macro x [] 1)",
        "(macros {:x (fn [] 1)})",
        "(eval-compiler (while true (pcall (fn [] (while true)))))",
        "(eval-compiler (string.rep \"\" 9223372036854775807))",
        "(eval-compiler (io.open \"canary.fnl\"))",
        "(lua \"return 42\")",
        "`(+ 1 2)",
        "(-> :canary (include))",
        "(when true (eval-compiler (os.execute :never)))",
    ] {
        let f = fixture(source, "fnl");
        f.write("canary.fnl", "(error :CANARY-WAS-READ)");
        f.write("canary.lua", "error('CANARY-WAS-READ')");
        let before = fs::read(f.root.path().join("canary.fnl")).unwrap();
        let mut command = f.command(true);
        command
            .env("FENNEL_PATH", format!("{}/?.fnl", f.root.path().display()))
            .env(
                "FENNEL_MACRO_PATH",
                format!("{}/?.fnl", f.root.path().display()),
            )
            .env("LUA_PATH", format!("{}/?.lua", f.root.path().display()));
        let result = envelope(finish(command.spawn().unwrap()), 1);
        let message = result["error"].as_str().unwrap();
        assert!(
            message.contains("unsupported Fennel form"),
            "{source}: {message}"
        );
        assert!(message.contains("main.fnl:"), "{message}");
        assert!(!message.contains("CANARY-WAS-READ"), "{message}");
        assert_eq!(fs::read(f.root.path().join("canary.fnl")).unwrap(), before);
        assert!(f.calls().is_empty());
    }
    // No lexical word blacklist: comments, strings and data keys remain useful.
    let f = fennel(";; include eval-compiler\n{:include :eval-compiler :macro :import-macros}");
    assert_eq!(
        f.run(false, 0)["output"],
        json!({"include":"eval-compiler","macro":"import-macros"})
    );
}

#[test]
fn standard_forms_and_json_fidelity_with_safe_rest_destructuring() {
    let f = fennel(
        r#"
 (local [head & rest] [1 2 3])
 (local {:a a & other} {:a 1 :b 2})
 (local twice (fn [x] (* 2 x)))
 (local composed (fn [x] (-> x (+ 1) twice)))
 (local squares (icollect [_ i (ipairs [1 2 3])] (* i i)))
 (local matched (match [head rest] [1 [2 3]] :yes _ :no))
 (local (x y) (values 8 9))
 (var n 0)
 (each [_ v (ipairs squares)] (set n (+ n v)))
 (while (< n 15) (set n (+ n 1)))
 (assert (= nil (getmetatable (json.array))))
 (assert (= nil (getmetatable (json.object))))
 (assert (= nil (getmetatable :string)))
 (assert (= input.present json.null))
 (assert (= input.missing nil))
 {:head head :rest rest :other other :squares squares :matched matched
  :closure (composed 5) :multi [x y] :loop n :input input
  :array (json.array) :object (json.object) :plain []
  :lo math.mininteger :hi math.maxinteger :encoded (json.encode input)}
"#,
    );
    let input = json!({"present":null,"a":[],"o":{},"lo":i64::MIN,"hi":i64::MAX,"fraction":1.25});
    f.json("input.schema.json", json!(true));
    f.json("input.json", input.clone());
    let v = f.run(false, 0)["output"].clone();
    assert_eq!(v["input"], input);
    assert_eq!(v["rest"], json!([2, 3]));
    assert_eq!(v["other"], json!({"b":2}));
    assert_eq!(v["squares"], json!([1, 4, 9]));
    assert_eq!(v["matched"], "yes");
    assert_eq!(v["closure"], 12);
    assert_eq!(v["multi"], json!([8, 9]));
    assert_eq!(v["loop"], 15);
    assert_eq!(v["array"], json!([]));
    assert_eq!(v["plain"], json!({}));
    assert_eq!(v["lo"], json!(i64::MIN));
    assert_eq!(v["hi"], json!(i64::MAX));
    assert_eq!(
        serde_json::from_str::<Value>(v["encoded"].as_str().unwrap()).unwrap(),
        input
    );
}

#[test]
fn invalid_json_and_handles_cannot_escape_checks() {
    for value in [
        "nil",
        "(/ 0 0)",
        "(/ 1 0)",
        "9007199254740992.0",
        "{1 1 :a 2}",
        "{2 1}",
        "(fn [] 1)",
        "(do (local t {}) (tset t :self t) t)",
        "(json.array {:a 1})",
        "(string.char 255)",
    ] {
        let f = fennel(value);
        error(&f, false);
    }
    let f = fennel(
        "(local h (due.start {:id :a :profile :reader :prompt :hold})) (assert (= nil (getmetatable h))) h",
    );
    let v = f.run(false, 1);
    assert_eq!(v["steps"].as_array().unwrap().len(), 1);
    assert_native_cleanup(&f);
    let f = fennel("(setmetatable {} {})");
    assert!(error(&f, true).contains("setmetatable"));
}

#[test]
fn parser_macro_and_post_wait_callback_errors_have_verified_locations() {
    for (source, line) in [
        ("(fn [input]\n (+ 1 2)\n ]", 3),
        ("(fn [input]\n (let [a] a))", 2),
    ] {
        let f = fixture(source, "fnl");
        let message = error(&f, true);
        assert!(message.contains(&format!("main.fnl:{line}:")), "{message}");
    }
    for body in ["(due.wait {})", "(error :after-wait)"] {
        let f = fixture(
            "local c=require('local/outer'); return function(input) return c.go() end",
            "lua",
        );
        f.write(
            "outer.fnl",
            "(local inner (require :local/inner)) {:go (fn [] (inner.go))}",
        );
        fs::create_dir(f.root.path().join("nested")).unwrap();
        f.write("nested/inner.fnl", &format!("{{:go (fn []\n  (local h (due.start {{:id :a :profile :reader :prompt :barrier-a}}))\n  (due.wait h)\n  (var x 0)\n  (for [i 1 800] (set x (+ x i)))\n  {body})}}"));
        f.edit(|m| {
            m["modules"] = json!({"local/outer":"outer.fnl","local/inner":"nested/inner.fnl"})
        });
        let mut child = f.command(false).spawn().unwrap();
        wait_for(&f.root.path().join("a-ready"), &mut child);
        f.write("a-release", "go");
        let v = envelope(finish(child), 1);
        let message = v["error"].as_str().unwrap();
        let snapshot = f.read("artifacts/snapshot.json");
        let compiled = &snapshot["fennel"]["compiled"]["nested/inner.fnl"];
        let generated = compiled["lua"]
            .as_str()
            .unwrap()
            .lines()
            .position(|line| {
                line.contains(if body == "(due.wait {})" {
                    "due.wait({})"
                } else {
                    "after-wait"
                })
            })
            .unwrap()
            + 1;
        assert_ne!(
            generated, 6,
            "fixture must distinguish source and generated lines"
        );
        assert_eq!(
            compiled["map"][generated - 1],
            json!(["nested/inner.fnl", 6])
        );
        assert!(
            message.contains(&format!(
                "nested/inner.fnl:6 (generated Lua nested/inner.fnl:{generated})"
            )),
            "{message}"
        );
        assert_eq!(v["steps"][0]["ok"], true);
        assert_native_cleanup(&f);
    }
}

#[test]
fn source_depth_and_bundle_limits_fail_before_agents() {
    let f = fixture(&" ".repeat(128 * 1024 + 1), "fnl");
    assert!(error(&f, true).contains("128 KiB"));
    let f = fixture(
        &format!("{}1{}", "(+ 1 ".repeat(100), ")".repeat(100)),
        "fnl",
    );
    assert!(error(&f, true).contains("depth budget"));
    let f = fennel("true");
    for i in 0..5 {
        f.write(&format!("m{i}.lua"), &" ".repeat(128 * 1024));
    }
    f.edit(|m| m["modules"] = json!({"local/a":"m0.lua","local/b":"m1.lua","local/c":"m2.lua","local/d":"m3.lua","local/e":"m4.lua"}));
    assert!(error(&f, true).contains("512 KiB"));
}

#[test]
fn runtime_budget_early_return_error_and_signal_join_jobs() {
    for body in [
        "(while true (pcall (fn [] (while true))))",
        "(error :stop)",
        "true",
    ] {
        let f = fennel(&format!(
            "(due.start {{:id :slow :profile :reader :prompt :slow}}) (due.run {{:id :fast :profile :reader :prompt :fast}}) {body}"
        ));
        let v = f.run(false, 1);
        assert_eq!(v["steps"].as_array().unwrap().len(), 2);
        assert_eq!(v["steps"][0]["exit"]["code"], 130);
        assert_native_cleanup(&f);
    }
    let f = fennel("(due.run {:id :a :profile :reader :prompt :hold})");
    let mut child = f.command(false).spawn().unwrap();
    wait_for(&f.root.path().join("ready"), &mut child);
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let v = envelope(finish(child), 130);
    assert_eq!(v["status"], "cancelled");
    assert_native_cleanup(&f);
}

#[test]
fn vendored_provenance_plain_lua_and_embedded_guidance() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let hash = Command::new("shasum")
        .args(["-a", "256"])
        .arg(root.join("assets/vendor/fennel/fennel-1.6.1.lua"))
        .output()
        .unwrap();
    assert!(hash.status.success());
    assert!(
        String::from_utf8_lossy(&hash.stdout)
            .starts_with("c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2")
    );
    let f = fixture("return function(input) return input end", "lua");
    f.run(false, 0);
    assert!(f.read("artifacts/snapshot.json").get("fennel").is_none());
    let f = fixture("return 42", "lua");
    f.edit(|m| m["schema_version"] = json!(1));
    assert_eq!(f.run(false, 0)["output"], 42);
    let f = fennel("true");
    f.edit(|m| m["schema_version"] = json!(1));
    assert!(error(&f, true).contains("schema_version 2"));
    let f = fennel("input");
    let mut command = f.command(false);
    command
        .env("PATH", "/nonexistent")
        .env("FENNEL_PATH", "/nonexistent/?.fnl");
    let v = envelope(finish(command.spawn().unwrap()), 0);
    assert_eq!(v["output"], f.read("input.json"));
    assert!(f.calls().is_empty());
}

#[test]
fn compiler_memory_operation_expansion_and_generated_output_budgets() {
    for (source, expected) in [
        ("(pick-values 1e20 (values))".to_owned(), "memory"),
        (
            "(pick-values 60000 (values))".to_owned(),
            "operation budget",
        ),
        (
            "(pick-values 25000 (values))".to_owned(),
            "generated output budget",
        ),
        (
            format!("(fn [x] {} true)", "(+ 1 2) ".repeat(4200)),
            "expansion budget",
        ),
    ] {
        let f = fixture(&source, "fnl");
        let message = error(&f, true);
        assert!(message.contains(expected), "expected {expected}: {message}");
    }
}

#[test]
#[ignore = "requires native process inspection (/bin/ps); run explicitly outside a sandbox that denies it"]
fn signals_during_compilation_join_even_a_stopped_compiler() {
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};
    for signal in [libc::SIGINT, libc::SIGTERM] {
        for target in ["parent", "group", "helper"] {
            for check in [false, true] {
                let f = fixture(
                    &format!("(fn [x] {} true)", "(+ 1 2) ".repeat(14000)),
                    "fnl",
                );
                // Every negative-PID signal below is scoped to this workflow's group.
                let mut child = f.command(check).process_group(0).spawn().unwrap();
                let start = Instant::now();
                let compiler = loop {
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "exited before compiler readiness"
                    );
                    assert!(
                        start.elapsed() < Duration::from_secs(3),
                        "compiler never started"
                    );
                    let ps = Command::new("/bin/ps")
                        .args(["-axo", "pid=,ppid="])
                        .output()
                        .unwrap();
                    assert!(ps.status.success());
                    let found = String::from_utf8(ps.stdout)
                        .unwrap()
                        .lines()
                        .find_map(|line| {
                            let mut fields = line.split_whitespace();
                            let pid = fields.next()?.parse::<i32>().ok()?;
                            let parent = fields.next()?.parse::<u32>().ok()?;
                            (parent == child.id()).then_some(pid)
                        });
                    if let Some(pid) = found
                        && unsafe { libc::kill(pid, libc::SIGSTOP) } == 0
                    {
                        break pid;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                };
                let parent = child.id() as i32;
                assert_eq!(
                    unsafe { libc::getpgid(compiler) },
                    parent,
                    "helper left parent's group"
                );
                let destination = match target {
                    "group" => -parent,
                    "helper" => compiler,
                    _ => parent,
                };
                assert_eq!(unsafe { libc::kill(destination, signal) }, 0);
                if target != "parent" {
                    // A stopped helper needs SIGCONT to act on pending SIGINT/SIGTERM.
                    // The parent may already have killed/reaped it during group cleanup.
                    unsafe {
                        libc::kill(compiler, libc::SIGCONT);
                    }
                }
                let v = envelope(finish(child), 130);
                assert_eq!(v["status"], "cancelled");
                assert!(
                    v["error"]
                        .as_str()
                        .unwrap()
                        .contains("compilation cancelled")
                );
                assert_ne!(
                    unsafe { libc::kill(compiler, 0) },
                    0,
                    "compiler survived parent cancellation"
                );
                assert!(f.calls().is_empty());
                assert!(!f.root.path().join("artifacts").exists());
            }
        }
    }
}

#[test]
fn shipped_feature_examples_and_fennel_component_called_from_lua() {
    let f = fixture(include_str!("../examples/workflow/features.fnl"), "fnl");
    let v = f.run(false, 0)["output"].clone();
    assert_eq!(v["rest"], json!([2, 3]));
    assert_eq!(v["mapped"], json!({"a":2,"b":4}));
    assert_eq!(v["sum"], 14);
    assert_eq!(v["ranged"], json!([2, 4, 6]));
    assert_eq!(v["folded"], 6);
    assert_eq!(v["matched"], "yes");
    assert_eq!(v["cased"], "yes");
    assert_eq!(v["closure"], 22);
    assert_eq!(v["thread-last"], 10);
    assert_eq!(v["optional"], 4);
    assert_eq!(v["lambda"], 3);
    assert_eq!(v["doto"], json!({"answer":42}));
    let f = fixture(
        include_str!("../examples/workflow/fennel-from-lua.lua"),
        "lua",
    );
    f.write(
        "component.fnl",
        include_str!("../examples/workflow/component.fnl"),
    );
    f.edit(|m| m["modules"] = json!({"local/component":"component.fnl"}));
    assert_eq!(f.run(false, 0)["output"]["approved"], true);
    assert_eq!(
        f.read("artifacts/steps/review/step.json")["phase"],
        "review"
    );
    for (language, source) in [
        ("fnl", include_str!("../examples/workflow/arrival.fnl")),
        ("lua", include_str!("../examples/workflow/arrival-v2.lua")),
    ] {
        let f = fixture(source, language);
        f.json(
            "output.schema.json",
            serde_json::from_str(include_str!("../examples/workflow/outcomes.schema.json"))
                .unwrap(),
        );
        f.run(true, 0);
        let v = f.run(false, 0);
        assert_eq!(v["steps"].as_array().unwrap().len(), 3);
        assert_eq!(v["output"]["replacement"]["ok"], true);
        assert_native_cleanup(&f);
    }
}

#[test]
fn cancel_active_job_preserves_a_live_sibling_in_both_v2_languages() {
    for (language, source) in [
        (
            "fnl",
            r#"(fn [input]
          (local a (due.start {:id :a :profile :reader :prompt :barrier-a}))
          (local b (due.start {:id :b :profile :reader :prompt :barrier-b}))
          (local ready (due.start {:id :ready :profile :reader :prompt :barrier-ready}))
          (due.wait ready) (due.cancel a) (due.wait b))"#,
        ),
        (
            "lua",
            r#"return function(input)
          local a=due.start({id='a',profile='reader',prompt='barrier-a'})
          local b=due.start({id='b',profile='reader',prompt='barrier-b'})
          local ready=due.start({id='ready',profile='reader',prompt='barrier-ready'})
          due.wait(ready); due.cancel(a); return due.wait(b)
        end"#,
        ),
    ] {
        let f = fixture(source, language);
        f.edit(|m| m["max_concurrency"] = json!(3));
        let mut child = f.command(false).spawn().unwrap();
        for name in ["a-ready", "b-ready", "ready-ready"] {
            wait_for(&f.root.path().join(name), &mut child);
        }
        f.write("ready-release", "go");
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
        assert_native_cleanup(&f);
    }
}
