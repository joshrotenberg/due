#![cfg(all(unix, feature = "workflow-quickjs-spike"))]
#[path = "support/workflow.rs"]
mod support;
use serde_json::{Value, json};
use std::{fs, thread, time::Duration};
use support::*;

fn js(source: &str) -> Fixture {
    let f = Fixture::new("");
    f.write("main.js", source);
    f.edit(|m| {
        m["entry"] = json!("main.js");
        m["schema_version"] = json!(2);
    });
    f
}
fn body(source: &str) -> Fixture {
    js(&format!(
        "export default async function(due,input) {{ {source} }}"
    ))
}
fn run(f: &Fixture, code: i32) -> Value {
    Running::new(f).finish(code)
}
fn events(f: &Fixture) -> Vec<Value> {
    fs::read_to_string(f.root.path().join("artifacts/workflow.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn signal_parent(f: &Fixture, signal: i32) {
    let pid = fs::read_to_string(f.root.path().join("parent-pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
}
fn signallable(f: &Fixture) -> Running {
    // The exec wrapper records only our own workflow PID; exec preserves it.
    let mut command = std::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("printf '%s\\n' \"$$\" > \"$1\"; shift; exec \"$@\"")
        .arg("workflow-test")
        .arg(f.root.path().join("parent-pid"))
        .arg(env!("CARGO_BIN_EXE_due"))
        .args(["workflow", "run"])
        .arg(f.root.path().join("workflow.json"))
        .args(["--input", "input.json", "--output", "json", "--artifacts"])
        .arg(f.root.path().join("artifacts"))
        .env("DUE_WORKFLOW_TEST_DIR", f.root.path());
    Running::with_command(f, command)
}

#[test]
fn json_preserves_arrays_objects_null_absence_and_safe_numbers() {
    let f = body(
        r#"
        if ('absent' in input || input.present !== null) throw Error('presence');
        return {input, emptyArray:[], emptyObject:{}, null:null,
          lo:Number.MIN_SAFE_INTEGER, hi:Number.MAX_SAFE_INTEGER, fraction:1.25};
    "#,
    );
    let input = json!({"present":null,"array":[],"object":{},"nested":[{"value":null}]});
    f.json("input.schema.json", json!(true));
    f.json("input.json", input.clone());
    let v = run(&f, 0);
    assert_eq!(v["output"]["input"], input);
    assert_eq!(v["output"]["emptyArray"], json!([]));
    assert_eq!(v["output"]["emptyObject"], json!({}));
    assert_eq!(v["output"]["null"], Value::Null);
    assert_eq!(v["output"]["lo"], json!(-9_007_199_254_740_991i64));
    assert_eq!(v["output"]["hi"], json!(9_007_199_254_740_991i64));
    assert_eq!(v["output"]["fraction"], json!(1.25));
    assert!(f.calls().is_empty());
    let f = body(
        "const r = await due.run({id:'null',profile:'reader',prompt:'null'}); return {present:r.answer_present,answer:r.answer};",
    );
    f.json("answer.schema.json", json!({"type":"null"}));
    assert_eq!(run(&f, 0)["output"], json!({"present":true,"answer":null}));
}

#[test]
fn unsupported_json_is_rejected_instead_of_silently_serialized() {
    for source in [
        "return undefined",
        "return {value:undefined}",
        "return NaN",
        "return Infinity",
        "return 9007199254740992",
        "return 1n",
        "return ()=>true",
        "return Symbol('x')",
        "return [1,,3]",
        "const a=[]; a.extra=true; return a",
        "const a={}; a.self=a; return a",
        "return new Date()",
        "return Object.create({inherited:1})",
        "return {get value(){return 1}}",
        "return {toJSON(){return true}}",
        "return {[Symbol('x')]:1}",
        "return Array(4097).fill(true)",
        "const o={}; for(let i=0;i<257;i++)o['k'+i]=i; return o",
        "let a=true; for(let i=0;i<34;i++)a=[a]; return a",
        "return 'x'.repeat(1048577)",
    ] {
        let f = body(source);
        let v = run(&f, 1);
        assert_eq!(v["status"], "failed", "{source}: {v}");
        assert!(f.calls().is_empty(), "{source}");
    }
    for value in [
        json!(9_007_199_254_740_992u64),
        json!(i64::MIN),
        json!(u64::MAX),
    ] {
        let f = body("return input");
        f.json("input.schema.json", json!(true));
        f.json("input.json", value);
        run(&f, 1);
        assert!(f.calls().is_empty());
    }
}

#[test]
fn mutated_intrinsics_cannot_hide_sparse_arrays_from_json_validation() {
    let f = body("globalThis.String=()=> 'foo'; const a=Array(1); a.foo=7; return a;");
    let value = run(&f, 1);
    assert_eq!(value["status"], "failed");
    assert!(f.calls().is_empty());
}

#[test]
fn check_compiles_every_declared_module_without_running_initializers() {
    let f = js("import {unused} from 'local/m'; export default async function(){return true}");
    f.write("module.js", "while(true){}; export const unused=1;");
    f.edit(|m| m["modules"] = json!({"local/m":"module.js"}));
    f.run(true, 0);
    assert!(!f.root.path().join("artifacts").exists());
    assert!(f.calls().is_empty());
    f.write("module.js", "export const = ;");
    f.run(true, 1);
    assert!(f.calls().is_empty());
    let f = js("export default async function(){return true}; throw Error('initializer');");
    f.run(true, 0);
    run(&f, 1);
    assert!(f.calls().is_empty());
}

const PIPELINE: &str = r#"
export async function pipeline(due, prefix, profile, first) {
  due.phase(prefix+'-design');
  const design = await due.run({id:prefix+'-design',profile,prompt:first});
  if (!design.ok) return design;
  due.phase(prefix+'-implement');
  const implementation = await due.run({id:prefix+'-implement',profile,prompt:'implement:'+prefix});
  due.phase(prefix+'-review');
  return due.run({id:prefix+'-review',profile,prompt:'review:'+prefix});
}
"#;
fn pipeline(f: &Fixture) {
    f.write("pipeline.js", PIPELINE);
    f.edit(|m| m["modules"] = json!({"local/pipeline":"pipeline.js"}));
}
fn workspaces(f: &Fixture) {
    for name in ["workspace-a", "workspace-b"] {
        fs::create_dir(f.root.path().join(name)).unwrap();
        f.write(&format!("{name}/sentinel"), "unchanged\n");
    }
    f.edit(|m| {
        for name in ["a", "b"] {
            let mut profile = m["profiles"]["reader"].clone();
            profile["request"]["cwd"] = json!(format!("workspace-{name}"));
            m["profiles"][name] = profile;
        }
    });
    f.write("agent", &include_str!("fixtures/workflow-agent.sh").replacen(
        "case \"$prompt\" in",
        r#"case "$prompt" in
    design:*|implement:*)
        stage=${prompt%%:*}; workspace=${PWD##*/}
        printf '%s\n' "$stage" >> effects.txt
        printf ready > "$DUE_WORKFLOW_TEST_DIR/$workspace-$stage-ready"
        while [ ! -f "$DUE_WORKFLOW_TEST_DIR/$workspace-$stage-release" ]; do /bin/sleep 0.01; done ;;
esac
case "$prompt" in"#, 1));
}

#[test]
fn unchanged_imported_pipelines_overlap_and_keep_effects_and_attribution() {
    let f = js(r#"
      import {pipeline} from 'local/pipeline';
      export default async function(due) {
        due.phase('root');
        const results=await due.concurrent('pipelines',[
          child=>pipeline(child,'a','a','design:a'),
          child=>pipeline(child,'b','b','design:b')]);
        due.log('root resumed');
        const synthesis=await due.run({id:'synthesis',profile:'reader',prompt:'synthesis'});
        return {results,synthesis};
      }
    "#);
    pipeline(&f);
    workspaces(&f);
    let mut running = Running::new(&f);
    for stage in ["design", "implement"] {
        running.ready(&format!("workspace-a-{stage}-ready"));
        running.ready(&format!("workspace-b-{stage}-ready"));
        for workspace in ["workspace-a", "workspace-b"] {
            f.write(&format!("{workspace}-{stage}-release"), "go");
        }
    }
    let result = running.finish(0);
    assert_eq!(result["steps"].as_array().unwrap().len(), 7);
    assert_eq!(result["output"]["results"][0]["id"], "a-review");
    assert_eq!(result["output"]["results"][1]["id"], "b-review");
    for name in ["workspace-a", "workspace-b"] {
        assert_eq!(
            fs::read_to_string(f.root.path().join(name).join("effects.txt")).unwrap(),
            "design\nimplement\n"
        );
        assert_eq!(
            fs::read_to_string(f.root.path().join(name).join("sentinel")).unwrap(),
            "unchanged\n"
        );
        assert_eq!(fs::read_dir(f.root.path().join(name)).unwrap().count(), 2);
    }
    assert!(!f.root.path().join("effects.txt").exists());
    for step in result["steps"].as_array().unwrap() {
        let id = step["id"].as_str().unwrap();
        let expected_scope = if id.starts_with("a-") {
            1
        } else if id.starts_with("b-") {
            2
        } else {
            0
        };
        assert_eq!(step["scope"]["id"], expected_scope);
        assert_eq!(step["phase"], if id == "synthesis" { "root" } else { id });
        for name in ["admitted", "started", "exit", "step"] {
            assert_eq!(
                f.read(&format!("artifacts/steps/{id}/{name}.json"))["scope"],
                step["scope"]
            );
        }
    }
    for event in events(&f).iter().filter(|event| event["event"] == "log") {
        assert_eq!(event["scope"]["id"], 0);
        assert_eq!(event["phase"], "root");
    }
    assert_native_cleanup(&f);
}

#[test]
fn custom_stages_and_nested_composition_preserve_order_and_failure_outcomes() {
    let f = body(
        r#"
      async function pipeline(ctx,prefix,stage) {
        const result=await stage(ctx,prefix);
        return {prefix,ok:result.ok,id:result.id};
      }
      const sequential=await pipeline(due,'seq',(ctx,id)=>ctx.run({id,profile:'reader',prompt:'invalid'}));
      const results=await due.concurrent('outer',[
        ctx=>pipeline(ctx,'slow',async (child,id)=>(await child.concurrent('nested',[
          nested=>nested.run({id,profile:'reader',prompt:'barrier-a'})]))[0]),
        ctx=>pipeline(ctx,'fast',(child,id)=>child.run({id,profile:'reader',prompt:'fail'}))]);
      return {sequential,results};
    "#,
    );
    let mut running = Running::new(&f);
    running.ready("a-ready");
    running.receipt("artifacts/steps/fast/step.json");
    assert!(
        !f.root
            .path()
            .join("artifacts/steps/slow/step.json")
            .exists()
    );
    f.write("a-release", "go");
    let value = running.finish(0);
    assert_eq!(value["output"]["sequential"]["ok"], false);
    assert_eq!(
        value["output"]["results"],
        json!([
      {"prefix":"slow","ok":true,"id":"slow"},
      {"prefix":"fast","ok":false,"id":"fast"}])
    );
    assert_eq!(f.calls().len(), 3);
}

#[test]
fn scope_identity_survives_interleaved_promises() {
    let f = body(
        r#"
      due.phase('root');
      const results=await due.concurrent('g',[
        async child=>{ child.phase('alpha'); await child.run({id:'a',profile:'reader',prompt:'barrier-a'});
          await Promise.resolve(); child.log('alpha resumed'); return child.run({id:'a2',profile:'reader',prompt:'ordinary'}); },
        async child=>{ child.phase('beta'); await child.run({id:'b',profile:'reader',prompt:'barrier-b'});
          await Promise.resolve(); child.log('beta resumed'); return true; }]);
      due.log('root resumed'); return results;
    "#,
    );
    let mut running = Running::new(&f);
    running.ready("a-ready");
    running.ready("b-ready");
    f.write("a-release", "go");
    running.receipt("artifacts/steps/a2/step.json");
    f.write("b-release", "go");
    running.finish(0);
    for event in events(&f)
        .iter()
        .filter(|event| event["event"] == "step_completed" || event["event"] == "log")
    {
        let scope = event["scope"]["id"].as_u64().unwrap();
        assert_eq!(
            event["phase"],
            match scope {
                0 => "root",
                1 => "alpha",
                2 => "beta",
                _ => panic!("unexpected scope"),
            }
        );
    }
}

#[test]
fn caught_host_failures_remain_fatal_and_prevent_later_admission() {
    for operation in [
        "await due.run({id:'bad',profile:'missing',prompt:'never'})",
        "await due.parallel('g',[{id:'same',profile:'reader',prompt:'never'},{id:'same',profile:'reader',prompt:'never'}])",
        "await due.concurrent('g',[])",
        "await due.concurrent('g',[()=>undefined])",
        "await due.concurrent('g',[()=>true,42])",
    ] {
        let f = body(&format!(
            "try{{{operation}}}catch(e){{}}; try{{await due.run({{id:'after',profile:'reader',prompt:'must-not-launch'}})}}catch(e){{}}; return true;"
        ));
        run(&f, 1);
        assert!(f.calls().is_empty(), "{operation}");
    }
}

#[test]
fn throwing_error_formatter_cannot_erase_a_fatal_host_input_failure() {
    let f = body(
        r#"
      Error.prototype.toString=function(){throw 'format failed'};
      try{due.start(undefined)}catch(e){}
      return {ok:true};
    "#,
    );
    let value = run(&f, 1);
    assert_eq!(value["status"], "failed");
    assert!(f.calls().is_empty());
}

#[test]
fn exception_diagnostics_never_execute_user_accessors() {
    let f = body(
        r#"
      const error=Error('original-failure');
      Object.defineProperty(error,'stack',{get(){
        due.start({id:'diagnostic-job',profile:'reader',prompt:'hold'});
        return 'replaced-cause';
      }});
      throw error;
    "#,
    );
    let value = run(&f, 1);
    assert_eq!(value["status"], "failed");
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("original-failure"),
        "{value}"
    );
    assert_eq!(value["steps"], json!([]));
    assert!(f.calls().is_empty());
}

#[test]
fn dangling_jobs_and_abandoned_components_are_cancelled_and_joined() {
    for source in [
        "due.start({id:'a',profile:'reader',prompt:'hold'}); return true",
        "due.run({id:'a',profile:'reader',prompt:'hold'}); return true",
        "due.concurrent('g',[async child=>{await child.run({id:'a',profile:'reader',prompt:'hold'});return true}]); return true",
        "try{await due.concurrent('g',[async child=>{child.start({id:'a',profile:'reader',prompt:'hold'});throw Error('deliberate')}])}catch(e){}; return true",
    ] {
        let f = body(source);
        let v = run(&f, 1);
        assert_eq!(v["status"], "failed", "{source}: {v}");
        assert_native_cleanup(&f);
    }
}

#[test]
fn detached_microtask_chain_cannot_silently_skip_declared_agent_work() {
    let f = body(
        r#"
      Promise.resolve().then(()=>Promise.resolve()).then(()=>Promise.resolve())
        .then(()=>due.run({id:'late',profile:'reader',prompt:'hold'}));
      return true;
    "#,
    );
    let value = run(&f, 1);
    assert_eq!(value["status"], "failed");
    assert_native_cleanup(&f);
}

#[test]
fn handles_cannot_cross_function_scopes_even_after_completion() {
    for source in [
        "const h=due.start({id:'a',profile:'reader',prompt:'ordinary'}); await due.wait(h); return due.concurrent('g',[child=>child.wait(h)])",
        "let h; await due.concurrent('g',[async child=>{h=child.start({id:'a',profile:'reader',prompt:'ordinary'});return child.wait(h)}]); return due.wait(h)",
        "let h; return due.concurrent('g',[async child=>{h=child.start({id:'a',profile:'reader',prompt:'hold'});return child.wait(h)},async child=>{await Promise.resolve();return child.cancel(h)}])",
    ] {
        let f = body(source);
        let v = run(&f, 1);
        assert!(v["error"].as_str().unwrap().contains("scope"), "{v}");
        assert_native_cleanup(&f);
    }
}

#[test]
fn escaped_capabilities_cannot_admit_jobs_after_their_scope_closes() {
    let f = body(
        r#"
      let escaped;
      await due.concurrent('g',[async child=>{escaped=child;return true}]);
      try {await escaped.run({id:'late',profile:'reader',prompt:'must-not-launch'});}catch(e){}
      return true;
    "#,
    );
    let value = run(&f, 1);
    assert_eq!(value["status"], "failed");
    assert!(f.calls().is_empty());
    assert!(value["steps"].as_array().unwrap().is_empty());
}

#[test]
fn wait_any_rejects_forged_containers_instead_of_accepting_numeric_handle_ids() {
    for forged in [
        "{map:()=>[0]}",
        "Object.defineProperty([h],'map',{value:()=>[0]})",
    ] {
        let f = body(&format!(
            r#"
          const h=due.start({{id:'a',profile:'reader',prompt:'ordinary'}});
          await due.wait(h);
          try{{await due.wait_any({forged});}}catch(e){{}}
          return true;
        "#
        ));
        let value = run(&f, 1);
        assert_eq!(value["status"], "failed");
        assert_eq!(f.calls(), vec!["ordinary"]);
    }
}

#[test]
fn completion_driven_dispatch_can_replace_work_and_cancel_a_sibling() {
    let f = body(
        r#"
      const slow=due.start({id:'slow',profile:'reader',prompt:'barrier-slow'});
      const fast=due.start({id:'fast',profile:'reader',prompt:'barrier-fast'});
      const first=await due.wait_any([slow,fast]);
      if(first.id!=='fast') throw Error('completion order');
      const replacement=await due.run({id:'replacement',profile:'reader',prompt:'ordinary'});
      const stopped=await due.cancel(slow);
      const repeated=await due.wait(slow);
      return {first:first.id,replacement:replacement.id,stopped,repeated};
    "#,
    );
    let mut running = Running::new(&f);
    running.ready("slow-ready");
    running.ready("fast-ready");
    f.write("fast-release", "go");
    let value = running.finish(0);
    assert_eq!(value["output"]["first"], "fast");
    assert_eq!(value["output"]["replacement"], "replacement");
    assert_eq!(value["output"]["stopped"]["cancel_requested"], true);
    assert_eq!(value["output"]["stopped"]["ok"], false);
    assert_eq!(value["output"]["stopped"], value["output"]["repeated"]);
    assert_eq!(value["steps"].as_array().unwrap().len(), 3);
    assert_native_cleanup(&f);
}

#[test]
fn shared_capacity_and_total_runs_apply_through_nested_components() {
    let f = body(
        r#"
      return due.concurrent('outer',[
        child=>child.concurrent('inner',[nested=>nested.parallel('jobs',[
          {id:'a',profile:'reader',prompt:'barrier-a'}, {id:'b',profile:'reader',prompt:'barrier-b'}])]),
        child=>child.run({id:'c',profile:'reader',prompt:'barrier-c'})]);
    "#,
    );
    f.edit(|m| m["max_concurrency"] = json!(1));
    let mut running = Running::new(&f);
    // Which sibling admits first is intentionally not a language contract.
    for _ in 0..3 {
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let label = loop {
            if let Some(label) = ["a", "b", "c"].into_iter().find(|label| {
                f.root.path().join(format!("{label}-ready")).exists()
                    && !f.root.path().join(format!("{label}-release")).exists()
            }) {
                break label;
            }
            assert!(running.alive());
            assert!(std::time::Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        };
        let active = ["a", "b", "c"]
            .into_iter()
            .filter(|label| {
                f.root.path().join(format!("{label}-ready")).exists()
                    && !f.root.path().join(format!("{label}-release")).exists()
            })
            .count();
        assert_eq!(active, 1);
        f.write(&format!("{label}-release"), "go");
    }
    running.finish(0);
    let f = body(
        "await due.run({id:'a',profile:'reader',prompt:'ordinary'}); try{await due.concurrent('g',[child=>child.run({id:'b',profile:'reader',prompt:'never'})])}catch(e){}; return true;",
    );
    f.edit(|m| {
        m["max_runs"] = json!(1);
        m["max_concurrency"] = json!(1)
    });
    assert!(run(&f, 1)["error"].as_str().unwrap().contains("max_runs"));
    assert_eq!(f.calls(), vec!["ordinary"]);
}

#[test]
fn memory_and_component_limits_bound_zero_job_scripts() {
    for source in [
        "const values=[]; for(;;)values.push('x'.repeat(4096));",
        "async function nest(ctx){return ctx.concurrent('g',[nest])}; return nest(due);",
        "for(let i=0;i<257;i++)await due.concurrent('g',[async()=>true]); return true;",
        "return due.concurrent('g',Array(33).fill(async()=>true));",
    ] {
        let f = body(source);
        let value = run(&f, 1);
        assert_eq!(value["status"], "failed", "{source}: {value}");
        assert!(f.calls().is_empty());
    }
}

#[test]
fn signal_cancels_active_and_queued_jobs_and_preserves_existing_effects() {
    let f = body(
        r#"
      due.start({id:'a',profile:'a',prompt:'design:a'});
      try { await due.concurrent('g',[
        child=>child.run({id:'b',profile:'b',prompt:'never'})]); }catch(e){}
      return due.run({id:'after',profile:'reader',prompt:'must-not-launch'});
    "#,
    );
    workspaces(&f);
    f.edit(|m| m["max_concurrency"] = json!(1));
    let mut running = signallable(&f);
    running.ready("workspace-a-design-ready");
    running.receipt("artifacts/steps/b/admitted.json");
    signal_parent(&f, libc::SIGTERM);
    let v = running.finish(130);
    assert_eq!(v["status"], "cancelled");
    assert_eq!(
        f.read("artifacts/steps/b/exit.json")["exit"]["started"],
        false
    );
    assert_eq!(
        fs::read_to_string(f.root.path().join("workspace-a/effects.txt")).unwrap(),
        "design\n"
    );
    assert!(!f.root.path().join("workspace-b/effects.txt").exists());
    assert_eq!(f.calls(), vec!["design:a"]);
    assert_native_cleanup(&f);
}

#[test]
fn diagnostics_retain_module_filename_after_await() {
    let f = js(
        "import {broken} from 'local/broken'; export default async function(due){return broken(due)}",
    );
    f.write("broken.js","export async function broken(due){\n await due.run({id:'a',profile:'reader',prompt:'ordinary'});\n throw Error('deliberate-after-await');\n}\n");
    f.edit(|m| m["modules"] = json!({"local/broken":"broken.js"}));
    let v = run(&f, 1);
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("deliberate-after-await"), "{v}");
    assert!(error.contains("broken"), "{v}");
    assert_eq!(f.calls(), vec!["ordinary"]);
}

#[test]
fn bounded_loops_and_unhandled_rejections_cannot_report_success() {
    for source in [
        "while(true){}",
        "while(true){try{for(;;){}}catch(e){}}",
        "while(true){await Promise.resolve()}",
        "function spin(){Promise.resolve().then(spin)}; spin(); return new Promise(()=>{})",
        "Promise.reject(Error('unhandled-fixture')); return true",
    ] {
        let f = body(source);
        run(&f, 1);
        assert!(f.calls().is_empty());
    }
}

#[test]
fn ordinary_handled_javascript_rejections_remain_recoverable() {
    for source in [
        "return Promise.reject(Error('expected')).catch(()=>({recovered:true}))",
        "try {await Promise.reject(Error('expected'));}catch(e){return {recovered:true}}",
        "const failed=Promise.reject(Error('expected')); await Promise.resolve(); return failed.catch(()=>({recovered:true}));",
    ] {
        let f = body(source);
        assert_eq!(run(&f, 0)["output"], json!({"recovered":true}));
        assert!(f.calls().is_empty());
    }
}

#[test]
fn promise_with_no_remaining_work_has_a_defined_failure() {
    let f = body("return new Promise(()=>{})");
    let value = run(&f, 1);
    assert_eq!(value["status"], "failed");
    let error = value["error"].as_str().unwrap();
    assert!(
        error.contains("pending") || error.contains("settle") || error.contains("progress"),
        "{value}"
    );
    assert!(f.calls().is_empty());
}

#[test]
fn signal_interrupts_pure_javascript_computation() {
    let f = body("due.log('spin-ready'); while(true){}");
    let mut running = signallable(&f);
    let until = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let log =
            fs::read_to_string(f.root.path().join("artifacts/workflow.jsonl")).unwrap_or_default();
        if log.contains("spin-ready") {
            break;
        }
        assert!(
            running.alive(),
            "workflow exited before entering pure JS loop"
        );
        assert!(std::time::Instant::now() < until, "no loop readiness event");
        thread::sleep(Duration::from_millis(1));
    }
    signal_parent(&f, libc::SIGTERM);
    let value = running.finish(130);
    assert_eq!(value["status"], "cancelled");
    assert!(f.calls().is_empty());
}

#[test]
fn quiet_native_wait_does_not_exhaust_script_budget() {
    let f = body(
        "const r=await due.run({id:'quiet',profile:'reader',prompt:'barrier-quiet'}); return r.ok;",
    );
    let mut running = Running::new(&f);
    running.ready("quiet-ready");
    // Longer than the historical Lua async-wrapper accounting regression.
    thread::sleep(Duration::from_secs(18));
    assert!(running.alive());
    f.write("quiet-release", "go");
    assert_eq!(running.finish(0)["output"], true);
    assert_native_cleanup(&f);
}
