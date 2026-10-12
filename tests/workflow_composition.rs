#![cfg(all(unix, feature = "workflow-lua-spike"))]
#[path = "support/workflow.rs"]
mod support;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::Child,
    thread,
    time::{Duration, Instant},
};
use support::*;

fn lua(source: &str) -> Fixture {
    let f = Fixture::new("");
    f.write("main.lua", source);
    f.edit(|m| m["entry"] = json!("main.lua"));
    f
}
// Own every spawned parent even if a barrier assertion panics. TERM lets Rust
// join its children. Emergency fixture-only cleanup is bounded and keeps evidence.
struct Running {
    child: Option<Child>,
    root: PathBuf,
}
impl Running {
    fn new(f: &Fixture) -> Self {
        // Large-answer regressions can exceed a pipe buffer in the final
        // envelope. Capture to fixture files so waiting cannot block on stdout.
        let mut command = f.command(false);
        command.stdout(fs::File::create(f.root.path().join("test-stdout")).unwrap());
        command.stderr(fs::File::create(f.root.path().join("test-stderr")).unwrap());
        Self {
            child: Some(command.spawn().unwrap()),
            root: f.root.path().into(),
        }
    }
    fn ready(&mut self, file: &str) {
        wait_for(&self.root.join(file), self.child.as_mut().unwrap());
    }
    fn signal(&self, signal: i32) {
        assert_eq!(
            unsafe { libc::kill(self.child.as_ref().unwrap().id() as i32, signal) },
            0
        );
    }
    fn finish(mut self, code: i32) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        while self.child.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(Instant::now() < until, "composition test deadline");
            thread::sleep(Duration::from_millis(10));
        }
        let mut output = self.child.take().unwrap().wait_with_output().unwrap();
        output.stdout = fs::read(self.root.join("test-stdout")).unwrap();
        output.stderr = fs::read(self.root.join("test-stderr")).unwrap();
        envelope(output, code)
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let until = Instant::now() + Duration::from_secs(10);
            while child.try_wait().unwrap().is_none() && Instant::now() < until {
                thread::sleep(Duration::from_millis(10));
            }
            if child.try_wait().unwrap().is_none() {
                // Only PIDs recorded by this synthetic fixture; no host inspection.
                for pid in fs::read_to_string(self.root.join("pids"))
                    .unwrap_or_default()
                    .lines()
                {
                    if let Ok(pid) = pid.parse::<i32>() {
                        unsafe {
                            libc::kill(pid, libc::SIGKILL);
                        }
                    }
                }
                for entry in fs::read_dir(self.root.join("artifacts/steps"))
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    if let Ok(bytes) = fs::read(entry.path().join("started.json"))
                        && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
                        && let Some(pid) = v["pid"].as_i64()
                    {
                        unsafe {
                            libc::kill(pid as i32, libc::SIGKILL);
                        }
                    }
                }
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}
fn run(f: &Fixture, code: i32) -> Value {
    Running::new(f).finish(code)
}
fn events(f: &Fixture) -> Vec<Value> {
    fs::read_to_string(f.root.path().join("artifacts/workflow.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
fn pipeline(f: &Fixture) {
    f.write(
        "pipeline.lua",
        include_str!("../examples/workflow/composition/pipeline.lua"),
    );
    f.edit(|m| m["modules"] = json!({"local/pipeline":"pipeline.lua"}));
}
fn workspaces(f: &Fixture) {
    for name in ["workspace-a", "workspace-b"] {
        fs::create_dir(f.root.path().join(name)).unwrap();
        f.write(&format!("{name}/sentinel"), "unchanged\n");
    }
    f.edit(|m| {
        let mut a = m["profiles"]["reader"].clone();
        a["request"]["cwd"] = json!("workspace-a");
        let mut b = a.clone();
        b["request"]["cwd"] = json!("workspace-b");
        m["profiles"]["workspace_a"] = a;
        m["profiles"]["workspace_b"] = b;
        m["profiles"]["synthesizer"] = m["profiles"]["reader"].clone();
    });
    let agent = include_str!("fixtures/workflow-agent.sh").replacen("case \"$prompt\" in", r#"case "$prompt" in
    design:*|implement:*)
        stage=${prompt%%:*}; workspace=${PWD##*/}
        printf '%s\n' "$stage" >> effects.txt
        printf ready > "$DUE_WORKFLOW_TEST_DIR/$workspace-$stage-ready"
        while [ ! -f "$DUE_WORKFLOW_TEST_DIR/$workspace-$stage-release" ]; do /bin/sleep 0.01; done ;;
esac
case "$prompt" in"#, 1);
    f.write("agent", &agent);
}
#[test]
fn two_unchanged_pipelines_overlap_and_synthesize_with_exact_profile_effects() {
    let f = lua(include_str!("../examples/workflow/composition/main.lua"));
    pipeline(&f);
    workspaces(&f);
    f.edit(|m| m["schema_version"] = json!(2));
    let mut child = Running::new(&f);
    for stage in ["design", "implement"] {
        child.ready(&format!("workspace-a-{stage}-ready"));
        child.ready(&format!("workspace-b-{stage}-ready"));
        // Both components reached this stage before either is released.
        for workspace in ["workspace-a", "workspace-b"] {
            f.write(&format!("{workspace}-{stage}-release"), "go");
        }
    }
    let v = child.finish(0);
    assert_eq!(v["steps"].as_array().unwrap().len(), 7);
    for (i, prefix) in ["a", "b"].iter().enumerate() {
        assert_eq!(v["output"]["outcomes"][i]["ok"], true);
        assert_eq!(
            v["output"]["outcomes"][i]["steps"][0]["id"],
            format!("{prefix}-design")
        );
    }
    assert_eq!(v["output"]["synthesis"]["ok"], true);
    for workspace in ["workspace-a", "workspace-b"] {
        assert_eq!(
            fs::read_to_string(f.root.path().join(workspace).join("effects.txt")).unwrap(),
            "design\nimplement\n"
        );
        assert_eq!(
            fs::read_to_string(f.root.path().join(workspace).join("sentinel")).unwrap(),
            "unchanged\n"
        );
        let mut files: Vec<_> = fs::read_dir(f.root.path().join(workspace))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        files.sort();
        assert_eq!(files, vec!["effects.txt", "sentinel"]);
    }
    assert!(!f.root.path().join("effects.txt").exists());
    let events = events(&f);
    for step in v["steps"].as_array().unwrap() {
        let id = step["id"].as_str().unwrap();
        let admitted = f.read(&format!("artifacts/steps/{id}/admitted.json"));
        let receipt = f.read(&format!("artifacts/steps/{id}/step.json"));
        assert_eq!(admitted["scope"], step["scope"]);
        assert_eq!(receipt["scope"], step["scope"]);
        for name in ["started", "exit"] {
            assert_eq!(
                f.read(&format!("artifacts/steps/{id}/{name}.json"))["scope"],
                step["scope"]
            );
        }
        for event in events.iter().filter(|e| e["data"]["step_id"] == id) {
            assert_eq!(event["phase"], step["phase"]);
            assert_eq!(event["scope"], step["scope"]);
        }
        assert!(
            fs::metadata(step["trace_file"].as_str().unwrap())
                .unwrap()
                .len()
                > 0
        );
        assert_eq!(
            step["scope"]["id"],
            if id.starts_with("a-") {
                1
            } else if id.starts_with("b-") {
                2
            } else {
                0
            }
        );
    }
    assert_native_cleanup(&f);
}

#[test]
fn injected_stage_nested_reuse_ordered_results_and_expected_failures() {
    let f = lua(r#"
        local p=require('local/pipeline')
        local function component(prefix, prompt)
            return p.pipeline({prefix=prefix,task='test',profiles={design='reader',implement='reader',review='reader'},
                stages={design=function(c,n,prior)
                    assert(#prior==0)
                    local result=due.concurrent('injected', {function()
                        return due.wait(due.start({id=c.prefix..'-override',profile='reader',prompt=prompt}))
                    end})
                    return result[1]
                end}})
        end
        local sequential=component('seq','invalid')
        local results=due.concurrent('outer', {
            function() return component('slow','barrier-a') end,
            function() return component('fast','fail') end
        })
        return {sequential=sequential,results=results}
    "#);
    pipeline(&f);
    let mut child = Running::new(&f);
    child.ready("a-ready");
    child.ready("artifacts/steps/fast-override/step.json");
    assert!(
        !f.root
            .path()
            .join("artifacts/steps/slow-override/step.json")
            .exists()
    );
    f.write("a-release", "go");
    let v = child.finish(0);
    assert_eq!(v["output"]["sequential"]["stopped_at"], "design");
    assert_eq!(v["output"]["results"][0]["ok"], true);
    assert_eq!(v["output"]["results"][1]["stopped_at"], "design");
    assert_eq!(v["steps"].as_array().unwrap().len(), 5);
    assert!(
        v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["scope"]["parent"].as_u64().unwrap_or(0) > 0)
    );
}

#[test]
fn capacity_and_runs_are_shared_across_nesting() {
    let f = lua(r#"
        return due.concurrent('outer', {
            function() return due.concurrent('inner', {function()
                return due.parallel('jobs',{{id='a',profile='reader',prompt='barrier-a'}, {id='b',profile='reader',prompt='barrier-b'}})
            end}) end,
            function() return due.run({id='c',profile='reader',prompt='barrier-c'}) end
        })
    "#);
    f.edit(|m| m["max_concurrency"] = json!(1));
    let mut child = Running::new(&f);
    child.ready("a-ready");
    child.ready("artifacts/steps/c/admitted.json");
    assert_eq!(f.calls(), vec!["barrier-a"]);
    f.write("a-release", "go");
    child.ready("b-ready");
    assert_eq!(f.calls(), vec!["barrier-a", "barrier-b"]);
    f.write("b-release", "go");
    child.ready("c-ready");
    f.write("c-release", "go");
    child.finish(0);
    let f = lua(r#"
        return due.concurrent('g', {function()
            due.run({id='a',profile='reader',prompt='ordinary'})
            return due.concurrent('nested',{function() return due.run({id='b',profile='reader',prompt='never'}) end})
        end, function() return true end})
    "#);
    f.edit(|m| {
        m["max_runs"] = json!(1);
        m["max_concurrency"] = json!(1);
    });
    assert!(run(&f, 1)["error"].as_str().unwrap().contains("max_runs"));
    assert_eq!(f.calls(), vec!["ordinary"]);
}

#[test]
fn per_scope_phase_is_preserved_after_sibling_changes_and_waits() {
    let f = lua(r#"
        due.phase('root')
        local v=due.concurrent('g', {
            function() due.phase('alpha'); due.run({id='a',profile='reader',prompt='barrier-a'});
                due.log('alpha resumed'); return due.run({id='a2',profile='reader',prompt='ordinary'}) end,
            function() due.phase('beta'); return due.run({id='b',profile='reader',prompt='barrier-b'}) end
        })
        due.log('root resumed'); return v
    "#);
    let mut child = Running::new(&f);
    child.ready("a-ready");
    child.ready("b-ready");
    f.write("a-release", "go");
    child.ready("artifacts/steps/a2/step.json");
    f.write("b-release", "go");
    child.finish(0);
    for e in events(&f) {
        if e["event"] == "step_completed" || e["event"] == "log" {
            let scope = e["scope"]["id"].as_u64().unwrap();
            assert_eq!(
                e["phase"],
                match scope {
                    0 => "root",
                    1 => "alpha",
                    2 => "beta",
                    _ => panic!(),
                }
            );
        }
    }
}

#[test]
fn dangling_return_and_errors_cancel_join_every_scope_even_when_protected() {
    for ending in [
        "return true",
        "error('deliberate')",
        "while true do pcall(function() while true do end end) end",
    ] {
        let f = lua(&format!(
            r#"
            pcall(function() pcall(function()
                due.concurrent('g', {{function()
                    due.start({{id='a',profile='reader',prompt='hold'}})
                    due.start({{id='q',profile='reader',prompt='never'}})
                    {ending}
                end, function() return true end}})
            end) end)
            return due.run({{id='after',profile='reader',prompt='must-not-launch'}})
        "#
        ));
        f.edit(|m| m["max_concurrency"] = json!(1));
        let v = run(&f, 1);
        assert_eq!(v["steps"].as_array().unwrap().len(), 2);
        assert_eq!(v["steps"][1]["exit"]["started"], false);
        assert!(!f.calls().iter().any(|p| p == "must-not-launch"));
        let es = events(&f);
        let last_job = es
            .iter()
            .rposition(|e| e["event"] == "step_completed")
            .unwrap();
        assert!(
            es.iter()
                .enumerate()
                .filter(|(_, e)| e["event"] == "scope_finished" && e["scope"]["id"] == 1)
                .all(|(i, _)| i > last_job)
        );
        assert_native_cleanup(&f);
    }
}

#[test]
fn signals_join_active_and_queued_jobs_preserving_partial_workspace_effects() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let f = lua(r#"
            local p=require('local/pipeline')
            pcall(function() pcall(function()
                due.concurrent('g', {function()
                    return p.pipeline({prefix='a',task='test',profiles={design='workspace_a',implement='workspace_a',review='workspace_a'}})
                end, function() return due.concurrent('nested',{function()
                    return due.run({id='queued',profile='workspace_b',prompt='never'})
                end}) end})
            end) end)
            return due.run({id='after',profile='reader',prompt='must-not-launch'})
        "#);
        pipeline(&f);
        workspaces(&f);
        f.edit(|m| m["max_concurrency"] = json!(1));
        let mut child = Running::new(&f);
        child.ready("workspace-a-design-ready");
        child.ready("artifacts/steps/queued/admitted.json");
        child.signal(signal);
        let v = child.finish(130);
        assert_eq!(v["status"], "cancelled");
        assert_eq!(v["steps"].as_array().unwrap().len(), 2);
        assert_eq!(v["steps"][1]["exit"]["started"], false);
        assert_eq!(
            fs::read_to_string(f.root.path().join("workspace-a/effects.txt")).unwrap(),
            "design\n"
        );
        assert!(!f.root.path().join("workspace-b/effects.txt").exists());
        for (workspace, count) in [("workspace-a", 2), ("workspace-b", 1)] {
            assert_eq!(
                fs::read_dir(f.root.path().join(workspace)).unwrap().count(),
                count
            );
        }
        for workspace in ["workspace-a", "workspace-b"] {
            assert_eq!(
                fs::read_to_string(f.root.path().join(workspace).join("sentinel")).unwrap(),
                "unchanged\n"
            );
        }
        assert!(
            !f.calls()
                .iter()
                .any(|p| p == "never" || p == "must-not-launch")
        );
        assert_native_cleanup(&f);
    }
}

#[test]
fn crossing_handles_is_fatal_in_every_direction_even_after_completion() {
    for source in [
        "local h=due.start({id='a',profile='reader',prompt='ordinary'}); due.wait(h); return due.concurrent('g',{function() return due.wait(h) end})",
        "local h; due.concurrent('g',{function() h=due.start({id='a',profile='reader',prompt='ordinary'}); return due.wait(h) end}); return due.wait(h)",
        "local h; return due.concurrent('g',{function() h=due.start({id='a',profile='reader',prompt='hold'}); return due.wait(h) end, function() return due.cancel(h) end})",
    ] {
        let f = lua(source);
        assert!(
            run(&f, 1)["error"]
                .as_str()
                .unwrap()
                .contains("another function scope")
        );
        assert_native_cleanup(&f);
    }
}

#[test]
fn invalid_inputs_results_names_and_zero_job_limits_are_sticky() {
    for call in [
        "due.concurrent('g',{})",
        "due.concurrent('g',{42})",
        "due.concurrent('g',{[1]=function() return true end,[3]=function() return true end})",
        "due.concurrent('g',{function() return true end,x=function() return true end})",
        "due.concurrent({}, {function() return true end})",
        "due.concurrent('bad/name',{function() return true end})",
        "due.concurrent('g',{function() end})",
        "due.concurrent('g',{function() return nil end})",
        "due.concurrent('g',{function() return 1,2 end})",
        "due.concurrent('g',{function() return function() end end})",
        "due.concurrent('g',{function() local t={}; t.self=t; return t end})",
        "local t={}; for i=1,33 do t[i]=function() return true end end; due.concurrent('g',t)",
    ] {
        let f = lua(&format!("pcall(function() {call} end); return true"));
        run(&f, 1);
        assert!(f.calls().is_empty());
    }
    for (source, expected) in [
        (
            "local function f() return due.concurrent('g',{f}) end; return f()",
            "depth exceeds",
        ),
        (
            "local function f() local t={}; for i=1,32 do t[i]=f end; return due.concurrent('g',t) end; return f()",
            "active continuations",
        ),
        (
            "for i=1,257 do due.concurrent('g',{function() return true end}) end; return true",
            "total continuations",
        ),
    ] {
        let f = lua(source);
        let v = run(&f, 1);
        assert!(v["error"].as_str().unwrap().contains(expected), "{v}");
        assert!(f.calls().is_empty());
    }
    let f = lua(
        "return due.concurrent('g',{function() return json.null end,function() return json.array() end,function() return 9007199254740993 end})",
    );
    assert_eq!(run(&f, 0)["output"], json!([null, [], 9007199254740993i64]));
}

#[test]
fn loading_is_serialized_effects_are_local_and_captured_state_is_shared() {
    let f = lua(r#"
        return due.concurrent('g', {
            function() local m=require('local/m'); return m.next() end,
            function() local r=due.run({id='a',profile='reader',prompt='ordinary'}); local m=require('local/m'); return m.next() end,
            function() local m=require('local/m'); return m.next() end
        })
    "#);
    f.write("module.lua","local sum=0; for i=1,2000 do sum=sum+i end; local n=0; return {next=function() n=n+1; return n end}");
    f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
    let v = run(&f, 0);
    let mut values: Vec<_> = v["output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    values.sort();
    assert_eq!(values, vec![1, 2, 3]);
    let f =
        lua("return due.concurrent('g',{function() pcall(require,'local/m'); return true end})");
    f.write(
        "module.lua",
        "pcall(function() due.concurrent('bad',{function() return true end}) end); return {}",
    );
    f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
    assert!(
        run(&f, 1)["error"]
            .as_str()
            .unwrap()
            .contains("effects unavailable")
    );
}

#[test]
fn pure_lua_stable_sort_and_shared_memory_instruction_budgets() {
    let f = lua(r#"
        local p=require('local/pipeline')
        local values={{key=2,id='a'},{key=1,id='b'},{key=2,id='c'}}
        local sorted=p.stable_sort(values,function(a,b) return a.key<b.key end)
        assert(values[1].id=='a')
        return sorted
    "#);
    pipeline(&f);
    assert_eq!(
        run(&f, 0)["output"],
        json!([{"key":1,"id":"b"},{"key":2,"id":"a"},{"key":2,"id":"c"}])
    );
    for source in [
        "local p=require('local/pipeline'); return due.concurrent('g',{function() return p.stable_sort({3,2,1},function() while true do pcall(function() while true do end end) end end) end})",
        "local held={}; local function f() held[#held+1]=string.rep('x',6*1024*1024); return true end; return due.concurrent('g',{f,f,f})",
        "local function f() local n=0; for i=1,14000 do n=n+i end; return n end; return due.concurrent('g',{f,f,f,f})",
    ] {
        let f = lua(source);
        pipeline(&f);
        run(&f, 1);
        assert!(f.calls().is_empty());
    }
}

#[cfg(feature = "workflow-fennel-spike")]
#[test]
fn fennel_composes_on_same_runtime_and_keeps_component_locations() {
    let f = lua("");
    f.write(
        "main.fnl",
        r#"(fn [input]
        (due.concurrent :g [(fn [] (due.run {:id :a :profile :reader :prompt :barrier-a}))
                            (fn [] (due.run {:id :b :profile :reader :prompt :barrier-b}))]))"#,
    );
    f.edit(|m| {
        m["schema_version"] = json!(2);
        m["entry"] = json!("main.fnl");
    });
    let mut child = Running::new(&f);
    child.ready("a-ready");
    child.ready("b-ready");
    f.write("b-release", "go");
    f.write("a-release", "go");
    let v = child.finish(0);
    assert_eq!(v["output"][0]["id"], "a");
    assert_eq!(v["output"][1]["id"], "b");
    let f = lua(
        "local a=require('local/a'); local b=require('local/b'); return due.concurrent('g',{a.run,b.run})",
    );
    f.write("a.fnl","{:run (fn []\n (due.run {:id :a :profile :reader :prompt :barrier-a})\n (error :deliberate))}");
    f.write(
        "b.fnl",
        "{:run (fn []\n (due.run {:id :b :profile :reader :prompt :barrier-b}))}",
    );
    f.edit(|m| m["modules"] = json!({"local/a":"a.fnl","local/b":"b.fnl"}));
    let mut child = Running::new(&f);
    child.ready("a-ready");
    child.ready("b-ready");
    f.write("a-release", "go");
    let v = child.finish(1);
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("last workflow location: a.fnl:3"), "{error}");
    assert!(!error.contains("last workflow location: b.fnl"));
    assert_native_cleanup(&f);
}

#[test]
fn early_return_and_parent_error_join_confirmed_active_descendants() {
    for ending in ["return true", "error('parent failure')"] {
        let f = lua(&format!(
            r#"
            return due.concurrent('outer', {{function()
                return due.concurrent('inner', {{function()
                    due.start({{id='slow',profile='reader',prompt='slow'}})
                    due.run({{id='ready',profile='reader',prompt='fast'}})
                    {ending}
                end}})
            end}})
        "#
        ));
        let v = run(&f, 1);
        assert!(f.root.path().join("slow-ready").exists());
        assert_eq!(v["steps"][0]["exit"]["code"], 130);
        let es = events(&f);
        let finished = es
            .iter()
            .position(|e| e["event"] == "step_completed" && e["data"]["step_id"] == "slow")
            .unwrap();
        assert!(
            es.iter()
                .enumerate()
                .filter(|(_, e)| e["event"] == "scope_finished")
                .all(|(i, _)| i > finished)
        );
        assert_native_cleanup(&f);
    }
}

#[test]
fn result_snapshots_and_combined_output_budget_and_profile_only_permissions() {
    let f = lua(
        "local shared={n=1}; return due.concurrent('g',{function() return shared end,function() shared.n=2; return shared end})",
    );
    assert_eq!(run(&f, 0)["output"], json!([{"n":1},{"n":2}]));
    let f = lua(
        "local function f() return string.rep('x',600000) end; pcall(function() due.concurrent('g',{f,f}) end); return true",
    );
    assert!(
        run(&f, 1)["error"]
            .as_str()
            .unwrap()
            .contains("results exceed")
    );
    for extra in [
        "cwd='elsewhere'",
        "native_args={'--sandbox','danger-full-access'}",
        "permissions='all'",
    ] {
        let f = lua(&format!(
            "return due.concurrent('g',{{function() return due.run({{id='a',profile='reader',prompt='never',{extra}}}) end}})"
        ));
        assert!(
            run(&f, 1)["error"]
                .as_str()
                .unwrap()
                .contains("unknown field")
        );
        assert!(f.calls().is_empty());
    }
    let f = lua("local n=0; for i=1,14000 do n=n+i end; return n");
    assert_eq!(run(&f, 0)["output"], 98007000);
}

#[test]
fn quiet_native_waits_do_not_spend_workflow_instructions() {
    let f = lua(r#"return due.concurrent('quiet', {
            function() return due.run({id='a',profile='reader',prompt='barrier-a'}) end,
            function() return due.run({id='b',profile='reader',prompt='barrier-b'}) end
        })"#);
    let mut child = Running::new(&f);
    child.ready("a-ready");
    child.ready("b-ready");
    // The former 5 ms polling workaround charged internal Lua wrappers and
    // exhausted the shared budget after about 16 seconds of otherwise idle work.
    thread::sleep(Duration::from_secs(20));
    assert!(
        child.child.as_mut().unwrap().try_wait().unwrap().is_none(),
        "quiet native waiting must not exhaust the workflow instruction budget"
    );
    f.write("a-release", "go");
    f.write("b-release", "go");
    let result = child.finish(0);
    assert!(
        result["output"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["ok"] == true)
    );
    assert_native_cleanup(&f);
}

#[test]
fn host_argument_conversion_failures_are_sticky_and_initialization_checks_run_first() {
    for call in [
        "due.parallel({}, {})",
        "due.phase({})",
        "due.log({})",
        "due.parallel(string.char(255), {})",
        "due.phase(string.char(255))",
        "due.log(string.char(255))",
    ] {
        for version in [1, 2] {
            for component in [false, true] {
                let body = format!(
                    "local ok=pcall(function() {call} end); assert(not ok); return due.run({{id='after',profile='reader',prompt='must-not-launch'}})"
                );
                let body = if component {
                    format!("return due.concurrent('g',{{function() {body} end}})")
                } else {
                    body
                };
                let f = lua(&if version == 2 {
                    format!("return function(input) {body} end")
                } else {
                    body
                });
                f.edit(|m| m["schema_version"] = json!(version));
                run(&f, 1);
                assert!(f.calls().is_empty(), "{call}");
                assert!(!f.root.path().join("artifacts/steps/after").exists());
            }
            // Bad arguments must not hide attempted effects in an initializer.
            let f = lua(&if version == 1 {
                "pcall(require,'local/m'); return true".into()
            } else {
                format!("pcall(function() {call} end); return function(input) return true end")
            });
            f.write(
                "module.lua",
                &format!("pcall(function() {call} end); return {{}}"),
            );
            f.edit(|m| {
                m["schema_version"] = json!(version);
                m["modules"] = json!({"local/m":"module.lua"});
            });
            let v = run(&f, 1);
            assert!(
                v["error"].as_str().unwrap().contains("effects unavailable"),
                "{v}"
            );
            assert!(f.calls().is_empty());
        }
    }
}

#[test]
fn require_recovery_is_limited_to_plain_v1_root_including_conversion_errors() {
    for argument in ["{}", "nil", "string.char(255)", "'missing'", "'local/m'"] {
        for version in [1, 2] {
            for component in [false, true] {
                let body = format!("assert(not pcall(require,{argument})); return true");
                let body = if component {
                    format!("return due.concurrent('g',{{function() {body} end}})")
                } else {
                    body
                };
                let f = lua(&if version == 2 {
                    format!("return function(input) {body} end")
                } else {
                    body
                });
                f.write("module.lua", "error('ordinary initializer failure')");
                f.edit(|m| {
                    m["schema_version"] = json!(version);
                    m["modules"] = json!({"local/m":"module.lua"});
                });
                run(&f, if version == 1 && !component { 0 } else { 1 });
                assert!(f.calls().is_empty());
            }
        }
    }
    #[cfg(feature = "workflow-fennel-spike")]
    {
        let f = lua("pcall(require,{}); return true");
        f.write("module.fnl", "{}");
        f.edit(|m| m["modules"] = json!({"local/m":"module.fnl"}));
        run(&f, 1); // Fennel loading policy also applies to a plain v1 root.
    }
}

#[test]
fn parallel_aggregate_conversion_failure_prevents_post_catch_admission() {
    let f = lua(r#"return function(input)
        return due.concurrent('g', {function()
            local jobs={}
            for i=1,3 do jobs[i]={id='large'..i,profile='reader',prompt='large-answer'} end
            assert(not pcall(due.parallel,'batch',jobs))
            return due.run({id='after',profile='reader',prompt='must-not-launch'})
        end})
    end"#);
    f.edit(|m| m["schema_version"] = json!(2));
    f.json("answer.schema.json", json!(true));
    let v = run(&f, 1);
    assert!(v["error"].as_str().unwrap().contains("JSON exceeds 1 MiB"));
    let steps = v["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 3);
    for step in steps {
        assert_eq!(step["ok"], true);
        assert_eq!(step["answer"]["payload"].as_str().unwrap().len(), 400_000);
    }
    assert_eq!(f.calls().len(), 3);
    assert!(!f.root.path().join("artifacts/steps/after").exists());
    assert_native_cleanup(&f);
}

#[test]
fn outcome_conversion_failure_is_sticky_for_run_wait_wait_any_and_cancel() {
    for op in ["run", "wait", "wait_any", "cancel"] {
        let body = if op == "run" {
            "assert(not pcall(due.run,{id='deep',profile='reader',prompt='deep-answer'}))".into()
        } else {
            format!(
                r#"local h=due.start({{id='deep',profile='reader',prompt='deep-answer'}})
                due.run({{id='gate',profile='reader',prompt='barrier-gate'}})
                assert(not pcall(due.{op},{}))"#,
                if op == "wait_any" { "{h}" } else { "h" }
            )
        };
        let f = lua(&format!(
            r#"return due.concurrent('g',{{function()
            {body}
            return due.run({{id='after',profile='reader',prompt='must-not-launch'}})
        end}})"#
        ));
        f.json("answer.schema.json", json!(true));
        let mut child = Running::new(&f);
        if op != "run" {
            child.ready("artifacts/steps/deep/step.json");
            child.ready("gate-ready");
            f.write("gate-release", "go");
        }
        let v = child.finish(1);
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("JSON nesting exceeds 32"),
            "{op}: {v}"
        );
        assert_eq!(v["steps"][0]["ok"], true); // Answer valid alone, too deep in its outcome.
        assert!(!f.root.path().join("artifacts/steps/after").exists());
        assert!(!f.calls().iter().any(|p| p == "must-not-launch"));
        assert_native_cleanup(&f);
    }
}

#[test]
fn component_depth_boundary_includes_the_aggregate_array() {
    for depth in [31, 32] {
        let value = format!("local v=true; for i=1,{depth} do v={{v}} end; return v");
        // Both individual values satisfy the global limit on their own.
        run(&lua(&value), 0);
        let f = lua(&format!(
            "local ok=pcall(due.concurrent,'g',{{function() {value} end}}); if not ok then return due.run({{id='after',profile='reader',prompt='must-not-launch'}}) end; return true"
        ));
        let v = run(&f, if depth == 31 { 0 } else { 1 });
        if depth == 32 {
            assert!(
                v["error"]
                    .as_str()
                    .unwrap()
                    .contains("JSON nesting exceeds 32")
            );
        }
        assert!(f.calls().is_empty());
        assert!(!f.root.path().join("artifacts/steps/after").exists());
    }
}

#[cfg(feature = "workflow-fennel-spike")]
#[test]
fn fennel_dangling_work_keeps_admitting_component_and_root_locations() {
    let f = lua("");
    f.write(
        "main.fnl",
        "(fn [input] (due.concurrent :g [(fn [] (due.start input.jobs.a) true) (fn [] true)]))",
    );
    f.edit(|m| {
        m["entry"] = json!("main.fnl");
        m["schema_version"] = json!(2);
    });
    f.json("input.schema.json", json!(true));
    f.json(
        "input.json",
        json!({"jobs":{"a":{"id":"a","profile":"reader","prompt":"hold"}}}),
    );
    let v = run(&f, 1);
    let error = v["error"].as_str().unwrap();
    assert!(
        error.contains("last workflow location: main.fnl:1"),
        "{error}"
    );
    assert_native_cleanup(&f);

    for component in [false, true] {
        let f = lua(if component {
            "local a=require('local/a'); local b=require('local/b'); return due.concurrent('g',{a.run,b.run})"
        } else {
            "local a=require('local/a'); return a.run()"
        });
        f.write("a.fnl", "{:run (fn []\n (due.start {:id :slow :profile :reader :prompt :slow})\n (due.run {:id :ready :profile :reader :prompt :fast})\n true)}");
        f.write(
            "b.fnl",
            "{:run (fn []\n (due.run {:id :sibling :profile :reader :prompt :barrier-b}))}",
        );
        f.edit(|m| m["modules"] = json!({"local/a":"a.fnl","local/b":"b.fnl"}));
        let v = run(&f, 1);
        let error = v["error"].as_str().unwrap();
        assert!(error.contains("outstanding jobs"), "{error}");
        assert!(error.contains("last workflow location: a.fnl:"), "{error}");
        assert!(!error.contains("last workflow location: b.fnl"), "{error}");
        if component {
            assert!(
                f.root
                    .path()
                    .join("artifacts/steps/sibling/admitted.json")
                    .exists()
            );
        }
        assert_native_cleanup(&f);
    }
}
