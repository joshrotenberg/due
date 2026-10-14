#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

static ID: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new(entry: &str) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/finocchio-acceptance")
            .join(format!(
                "{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::SeqCst)
            ));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let f = Self { root };
        for name in ["a", "b", "c"] {
            fs::create_dir(f.root.join(name)).unwrap();
        }
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workflow");
        let mut modules = serde_json::Map::new();
        for (name, file) in [
            ("reviewer", "reviewer"),
            ("reviser", "reviser"),
            ("cycle", "review-cycle"),
            ("parallel", "parallel"),
            ("pure", "pure"),
        ] {
            f.write(
                &format!("{name}.fnl"),
                &fs::read_to_string(examples.join(format!("{file}.fnl"))).unwrap(),
            );
            modules.insert(name.into(), json!(format!("{name}.fnl")));
        }
        f.write("due", include_str!("fixtures/due.py"));
        fs::set_permissions(f.root.join("due"), fs::Permissions::from_mode(0o700)).unwrap();
        f.json("config.json", json!({"entry":entry,"modules":modules,"due_binary":"due","profiles":{"review":{"provider":"codex","model":"synthetic-only","effort":"high","preferences":["careful"],"native_args":["--sandbox","read-only"]},"revise":{"provider":"codex","model":"synthetic-only","effort":"medium","preferences":["balanced"],"native_args":["--sandbox","workspace-write"]}},"roles":{"reviewer":{"module":"reviewer","profiles":["review"],"workspaces":["a","b","c"],"access":"read"},"reviser":{"module":"reviser","profiles":["revise"],"workspaces":["a","b","c"],"access":"write"}},"workspaces":{"a":"a","b":"b","c":"c"},"max_jobs":16,"max_active":2}));
        f.json("input.json", input("a", "approve", 1));
        f
    }
    fn write(&self, name: &str, value: &str) {
        fs::write(self.root.join(name), value).unwrap();
    }
    fn json(&self, name: &str, value: Value) {
        self.write(name, &value.to_string());
    }
    fn read(&self, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.root.join(name)).unwrap()).unwrap()
    }
    fn edit(&self, f: impl FnOnce(&mut Value)) {
        let mut v = self.read("config.json");
        f(&mut v);
        self.json("config.json", v);
    }
    fn script(&self, body: &str) {
        self.write("probe.fnl",&format!("(local meta (data.parse \"{{\\\"intent\\\":\\\"acceptance probe\\\",\\\"input_schema\\\":true,\\\"output_schema\\\":true}}\"))\n{body}"));
        self.edit(|c| {
            c["entry"] = json!("probe");
            c["modules"]["probe"] = json!("probe.fnl");
        });
    }
    fn command(&self, mode: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_finocchio-spike"));
        // Cwd is actually outside the checkout; every input/artifact is explicit.
        c.current_dir("/")
            .args(["workflow", mode])
            .arg(self.root.join("config.json"));
        if mode == "run" {
            c.arg(self.root.join("input.json"))
                .arg(self.root.join("artifacts"));
        }
        c.stdout(Stdio::from(
            fs::File::create(self.root.join("stdout")).unwrap(),
        ))
        .stderr(Stdio::from(
            fs::File::create(self.root.join("stderr")).unwrap(),
        ));
        c
    }
    fn start(&self, mode: &str) -> Running {
        Running {
            child: Some(self.command(mode).spawn().unwrap()),
            root: self.root.clone(),
        }
    }
    fn run(&self, code: i32) -> Value {
        self.start("run").finish(code)
    }
    fn calls(&self, ws: &str) -> Vec<Value> {
        fs::read_to_string(self.root.join(ws).join("calls"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}
struct Running {
    child: Option<Child>,
    root: PathBuf,
}
impl Running {
    fn ready(&mut self, path: &str) {
        let until = Instant::now() + Duration::from_secs(15);
        while !self.root.join(path).exists() {
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "exited waiting for {path}: {}",
                fs::read_to_string(self.root.join("stdout")).unwrap_or_default()
            );
            assert!(Instant::now() < until, "barrier deadline {path}");
            thread::sleep(Duration::from_millis(5));
        }
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
            assert!(
                Instant::now() < until,
                "CLI deadline; artifacts {}",
                self.root.display()
            );
            thread::sleep(Duration::from_millis(5));
        }
        let status = self.child.take().unwrap().wait().unwrap();
        let text = fs::read_to_string(self.root.join("stdout")).unwrap();
        assert_eq!(
            status.code(),
            Some(code),
            "{text}\nstderr: {}",
            fs::read_to_string(self.root.join("stderr")).unwrap()
        );
        serde_json::from_str(&text).unwrap()
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let until = Instant::now() + Duration::from_secs(3);
            while child.try_wait().unwrap().is_none() && Instant::now() < until {
                thread::sleep(Duration::from_millis(10));
            }
            if child.try_wait().unwrap().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        // Fixture-only emergency cleanup, never the product's cancellation path.
        for ws in ["a", "b", "c"] {
            let dir = self.root.join(ws);
            let _ = fs::write(dir.join("orphan-release"), "release");
            let _ = fs::write(dir.join("cleanup-release"), "release");
            for call in fs::read_to_string(dir.join("calls"))
                .unwrap_or_default()
                .lines()
            {
                if let Ok(v) = serde_json::from_str::<Value>(call)
                    && let Some(pid) = v["pid"].as_i64()
                {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }
            }
        }
    }
}
fn input(ws: &str, task: &str, max: usize) -> Value {
    json!({"workspace":ws,"task":task,"max_revisions":max})
}
fn status(v: &Value) -> &str {
    v["output"]["status"].as_str().unwrap()
}
fn stopped(v: &Value) {
    assert_eq!(v["status"], "failed", "{v}");
    assert!(v["error"]["message"].is_string());
}
fn settled(f: &Fixture, v: &Value) {
    for s in v["steps"].as_array().unwrap() {
        let id = s["id"].as_u64().unwrap();
        let prefix = format!("artifacts/job-{id:03}");
        assert_eq!(f.read(&format!("{prefix}/receipt.json"))["exit"], s["exit"]);
        assert!(f.root.join(format!("{prefix}/exit.json")).exists());
        if let Some(pid) = s["exit"]["pid"].as_i64() {
            assert_eq!(
                unsafe { libc::kill(pid as i32, 0) },
                -1,
                "Due child still alive"
            );
        }
    }
}

#[test]
fn pure_cli_from_outside_source_preserves_json_without_profiles_or_binary() {
    let f = Fixture::new("pure");
    f.edit(|c| {
        c["roles"] = json!({});
        c["profiles"] = json!({});
        c["due_binary"] = Value::Null;
        c["max_jobs"] = json!(0);
    });
    let value: Value =
        serde_json::from_str(include_str!("../examples/workflow/input.json")).unwrap();
    f.json("input.json", value.clone());
    let v = f.run(0);
    assert_eq!(v["output"], value);
    assert_eq!(v["steps"], json!([]));
}
#[test]
fn inspection_never_prepares_validates_or_runs_agents() {
    for mode in ["describe", "check"] {
        let f = Fixture::new("reviewer");
        let source = fs::read_to_string(f.root.join("reviewer.fnl"))
            .unwrap()
            .replace(
                "(host.validate :reviewer :input input)",
                "(error :run-sentinel)",
            ).replace(":prepare prepare :validate validate :run run", ":prepare (fn [x] (error :prepare-sentinel)) :validate (fn [x] (error :validate-sentinel)) :run run");
        f.write("reviewer.fnl", &source);
        f.edit(|c| {
            c["profiles"] = json!({});
            c["roles"] = json!({});
            c["due_binary"] = Value::Null;
        });
        let v = f.start(mode).finish(0);
        assert!(v["output"]["reviewer"]["metadata"]["skills"].is_array());
        assert!(f.calls("a").is_empty());
        assert!(!f.root.join("artifacts").exists());
    }
}
#[test]
fn initializer_attempts_even_caught_and_malformed_are_sticky_and_never_spawn() {
    for attempt in [
        "(host.run {})",
        "(host.run nil)",
        "(host.concurrent [])",
        "((. (require :reviewer) :run) (data.parse \"{}\"))",
    ] {
        for mode in ["run", "describe", "check"] {
            let f = Fixture::new("pure");
            f.script(&format!(
                "(pcall (fn [] {attempt}))\n{{:meta meta :run (fn [input] input)}}"
            ));
            let v = f.start(mode).finish(1);
            stopped(&v);
            assert!(
                v["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("effects unavailable"),
                "{v}"
            );
            assert!(f.calls("a").is_empty());
            assert_eq!(v["steps"], json!([]));
        }
    }
}
#[test]
fn direct_reviewer_and_bounded_revision_cycle_use_the_same_module() {
    let f = Fixture::new("reviewer");
    let v = f.run(0);
    assert_eq!(v["output"]["answer"]["verdict"], "approved");
    assert_eq!(f.calls("a").len(), 1);
    settled(&f, &v);
    let f = Fixture::new("cycle");
    f.json("input.json", input("a", "revise", 1));
    let v = f.run(0);
    assert_eq!(status(&v), "approved");
    assert_eq!(v["output"]["revisions"], 1);
    let calls = f.calls("a");
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls[1]["context"]["findings"],
        json!(["fix synthetic issue"])
    );
    assert_eq!(calls[2]["context"]["revision"], 1);
    assert_eq!(v["steps"][0]["admission"]["selected_profile"], "review");
    assert_eq!(
        v["steps"][1]["admission"]["selected_settings"]["native_args"],
        json!(["--sandbox", "workspace-write"])
    );
    assert!(v["steps"][0]["observed_model"].is_null());
    settled(&f, &v);
}
#[test]
fn approval_terminal_exhaustion_and_failed_mutation_never_replay() {
    for (task, max, expected, jobs) in [
        ("approve", 3, "approved", 1),
        ("endless", 0, "attempts_exhausted", 1),
        ("endless", 2, "attempts_exhausted", 5),
        ("failed-revision", 3, "native_failure", 2),
    ] {
        let f = Fixture::new("cycle");
        f.json("input.json", input("a", task, max));
        let v = f.run(0);
        assert_eq!(status(&v), expected);
        assert_eq!(v["steps"].as_array().unwrap().len(), jobs);
        settled(&f, &v);
    }
}
#[test]
fn execution_protocol_output_and_domain_failures_are_distinct() {
    for (task, expected) in [
        ("native-failure", "native_failure"),
        ("protocol", "protocol_error"),
        ("claimed-validation", "protocol_error"),
        ("malformed-envelope", "protocol_error"),
        ("invalid-output", "invalid_output"),
        ("missing-output", "invalid_output"),
        ("null-output", "invalid_output"),
    ] {
        let f = Fixture::new("cycle");
        f.json("input.json", input("a", task, 1));
        let v = f.run(0);
        assert_eq!(status(&v), expected);
        assert_eq!(f.calls("a").len(), 1);
        settled(&f, &v);
    }
}
#[test]
fn unchanged_workflows_overlap_reverse_completion_but_keep_input_order() {
    let f = Fixture::new("parallel");
    f.json(
        "input.json",
        json!([input("a", "barrier", 1), input("b", "barrier", 1)]),
    );
    let mut child = f.start("run");
    child.ready("a/ready");
    child.ready("b/ready");
    assert!(!f.root.join("artifacts/job-000/receipt.json").exists());
    assert!(!f.root.join("artifacts/job-001/receipt.json").exists());
    f.write("b/release", "go");
    child.ready("artifacts/job-001/receipt.json");
    assert!(!f.root.join("artifacts/job-000/receipt.json").exists());
    f.write("a/release", "go");
    let v = child.finish(0);
    assert_eq!(v["output"]["approved"], 2);
    assert_eq!(v["output"]["results"][0]["last"]["id"], 0);
    assert_eq!(v["output"]["results"][1]["last"]["id"], 1);
    assert_eq!(v["steps"][0]["completion_seq"], 1);
    assert_eq!(v["steps"][1]["completion_seq"], 0);
    settled(&f, &v);
}
#[test]
fn cancellation_closes_admission_joins_active_and_queued_even_through_pcall() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let f = Fixture::new("pure");
        f.script("(local r (require :reviewer))\n{:meta meta :run (fn [input] (pcall (fn [] (host.concurrent [(fn [] (r.run (data.get input 1))) (fn [] (r.run (data.get input 2))) (fn [] (r.run (data.get input 3)))]))) (r.run (data.get input 1)))}");
        f.json(
            "input.json",
            json!([
                input("a", "barrier", 1),
                input("b", "barrier", 1),
                input("c", "barrier", 1)
            ]),
        );
        let mut child = f.start("run");
        child.ready("a/ready");
        child.ready("b/ready");
        child.ready("artifacts/job-002/admitted.json");
        child.signal(signal);
        let v = child.finish(130);
        assert_eq!(v["status"], "cancelled");
        assert_eq!(v["steps"].as_array().unwrap().len(), 3);
        assert_eq!(v["steps"][2]["exit"]["started"], false);
        assert!(f.root.join("a/cleaned").exists());
        assert!(f.root.join("b/cleaned").exists());
        assert!(f.calls("c").is_empty());
        settled(&f, &v);
    }
}
#[test]
fn nested_concurrency_shares_active_and_total_caps() {
    let f = Fixture::new("pure");
    f.script("(local r (require :reviewer))\n{:meta meta :run (fn [input] (host.concurrent [(fn [] (host.concurrent [(fn [] (r.run (data.get input 1))) (fn [] (r.run (data.get input 2)))])) (fn [] (r.run (data.get input 3)))]))}");
    f.edit(|c| c["max_active"] = json!(1));
    f.json(
        "input.json",
        json!([
            input("a", "barrier", 1),
            input("b", "barrier", 1),
            input("c", "barrier", 1)
        ]),
    );
    let mut child = f.start("run");
    child.ready("a/ready");
    child.ready("artifacts/job-002/admitted.json");
    assert!(!f.root.join("b/ready").exists());
    assert!(!f.root.join("c/ready").exists());
    f.write("a/release", "go");
    child.ready("b/ready");
    assert!(!f.root.join("c/ready").exists());
    f.write("b/release", "go");
    child.ready("c/ready");
    f.write("c/release", "go");
    let v = child.finish(0);
    assert_eq!(v["steps"].as_array().unwrap().len(), 3);
    settled(&f, &v);
    let f = Fixture::new("parallel");
    f.edit(|c| c["max_jobs"] = json!(1));
    f.json(
        "input.json",
        json!([input("a", "barrier", 1), input("b", "barrier", 1)]),
    );
    let v = f.run(1);
    stopped(&v);
    assert_eq!(v["steps"].as_array().unwrap().len(), 1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("total job cap")
    );
    settled(&f, &v);
}
#[test]
fn canonical_workspace_relations_refuse_overlapping_writers_and_allow_readers() {
    for relation in ["same", "parent", "symlink"] {
        let f = Fixture::new("parallel");
        if relation == "symlink" {
            symlink(f.root.join("a"), f.root.join("alias")).unwrap();
        }
        if relation == "parent" {
            fs::create_dir(f.root.join("a/child")).unwrap();
        }
        f.edit(|c| {
            c["roles"]["reviewer"]["access"] = json!("write");
            c["workspaces"]["b"] = json!(match relation {
                "same" => "a",
                "parent" => "a/child",
                _ => "alias",
            });
        });
        f.json(
            "input.json",
            json!([input("a", "barrier", 1), input("b", "barrier", 1)]),
        );
        let v = f.run(1);
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("overlapping")
        );
        settled(&f, &v);
    }
    let f = Fixture::new("parallel");
    f.edit(|c| c["workspaces"]["b"] = json!("a"));
    f.json(
        "input.json",
        json!([input("a", "approve", 1), input("b", "approve", 1)]),
    );
    assert_eq!(f.run(0)["output"]["approved"], 2);
}
#[test]
fn scripts_cannot_override_profile_native_args_cwd_schema_or_role_grants() {
    for extra in [
        ":profile :revise",
        ":native_args [\"--danger\"]",
        ":cwd \"/\"",
        ":schema \"/tmp/arbitrary\"",
        ":permissions :all",
    ] {
        let f = Fixture::new("pure");
        f.script(&format!("{{:meta meta :run (fn [input] (pcall (fn [] (host.run {{:role :reviewer :workspace :a :input input :prompt :never {extra}}}))) ((. (require :reviewer) :run) input))}}"));
        let v = f.run(1);
        stopped(&v);
        assert!(f.calls("a").is_empty());
    }
    let f = Fixture::new("reviewer");
    f.edit(|c| c["roles"]["reviewer"]["workspaces"] = json!(["b"]));
    let v = f.run(1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not granted")
    );
    assert!(f.calls("a").is_empty());
}
#[test]
fn schemas_input_requests_and_model_selection_fail_before_children() {
    let f = Fixture::new("cycle");
    f.json("input.json", input("a", "approve", 4));
    stopped(&f.run(1));
    assert!(f.calls("a").is_empty());
    let f = Fixture::new("reviewer");
    f.edit(|c| c["profiles"]["review"]["preferences"] = json!(["fast"]));
    let v = f.run(1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no caller-granted profile")
    );
    assert!(f.calls("a").is_empty());
    let f = Fixture::new("reviewer");
    let s=fs::read_to_string(f.root.join("reviewer.fnl")).unwrap().replace("\\\"preference\\\":\\\"careful\\\"","\\\"exact\\\":{\\\"provider\\\":\\\"codex\\\",\\\"model\\\":\\\"synthetic-only\\\",\\\"effort\\\":\\\"high\\\"}");
    f.write("reviewer.fnl", &s);
    assert_eq!(
        f.run(0)["output"]["admission"]["selected_settings"]["model"],
        "synthetic-only"
    );
}
#[test]
fn hostile_loops_cycles_non_json_and_caught_errors_are_deadline_supervised() {
    for expr in [
        "(while true (pcall (fn [] (while true nil))))",
        "(fn [] true)",
        "(do (local t {}) (tset t :self t) t)",
        "nil",
        "(values true true)",
        "(string.rep \"\" 999999999999999)",
        "(table.move {} 1 99999999999999 1)",
        "(do (var t {:x :leaf}) (for [i 1 30] (set t {:a t :b t})) t)",
    ] {
        let f = Fixture::new("pure");
        f.script(&format!("{{:meta meta :run (fn [input] {expr})}}"));
        let code = if expr.starts_with("(string.rep") {
            0
        } else {
            1
        };
        let v = f.run(code);
        if code == 1 {
            stopped(&v);
        }
        assert!(f.calls("a").is_empty());
    }
}

#[test]
fn immutable_outcomes_preserve_null_empty_collections_and_large_numbers() {
    for task in ["huge-integer", "null-output"] {
        let f = Fixture::new("pure");
        // Bind a role to a module with a permissive *caller-visible* answer
        // contract for this round-trip test, not permission bypasses.
        f.write("roundtrip.fnl", "(local meta (data.parse \"{\\\"intent\\\":\\\"round trip\\\",\\\"input_schema\\\":true,\\\"output_schema\\\":true,\\\"answer_schema\\\":true}\"))\n{:meta meta :run (fn [input] (let [r (host.run {:role :reviewer :workspace :a :input input :prompt (data.json input)}) a (data.get r :answer)] (assert (= (type r) :userdata)) (assert (= (data.get r :absent) nil)) (assert (not (pcall (fn [] (tset r :ok false))))) a))}");
        f.edit(|c| {
            c["entry"] = json!("roundtrip");
            c["modules"]["roundtrip"] = json!("roundtrip.fnl");
            c["roles"]["reviewer"]["module"] = json!("roundtrip");
        });
        f.json("input.json", input("a", task, 0));
        let v = f.run(0);
        assert_eq!(v["steps"][0]["answer_present"], true);
        if task == "null-output" {
            assert!(v["output"].is_null());
        } else {
            let expected: Value = serde_json::from_str(
                "{\"big\":123456789012345678901234567890,\"null\":null,\"array\":[],\"object\":{}}",
            )
            .unwrap();
            assert_eq!(v["output"], expected);
        }
        settled(&f, &v);
    }
}

#[test]
fn entry_output_schema_and_offline_schema_subset_are_enforced() {
    let f = Fixture::new("pure");
    let source = fs::read_to_string(f.root.join("pure.fnl"))
        .unwrap()
        .replace(
            "\\\"output_schema\\\":true",
            "\\\"output_schema\\\":{\\\"type\\\":\\\"string\\\"}",
        );
    f.write("pure.fnl", &source);
    let v = f.run(1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("entry output")
    );
    assert!(f.calls("a").is_empty());
    let f = Fixture::new("pure");
    let source = fs::read_to_string(f.root.join("pure.fnl"))
        .unwrap()
        .replace(
            "\\\"output_schema\\\":true",
            "\\\"output_schema\\\":{\\\"$ref\\\":\\\"https://example.invalid/schema\\\"}",
        );
    f.write("pure.fnl", &source);
    let v = f.start("check").finish(1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unsupported schema keyword")
    );
}

#[test]
fn parent_signal_preserves_completed_and_interrupted_receipts() {
    let f = Fixture::new("pure");
    f.script("(local r (require :reviewer))\n{:meta meta :run (fn [input] (r.run (data.get input 1)) (pcall (fn [] (host.concurrent [(fn [] (r.run (data.get input 2))) (fn [] (r.run (data.get input 3)))]))) true)}");
    f.json(
        "input.json",
        json!([
            input("c", "approve", 0),
            input("a", "barrier", 0),
            input("b", "barrier", 0)
        ]),
    );
    let mut child = f.start("run");
    child.ready("a/ready");
    child.ready("b/ready");
    child.signal(libc::SIGTERM);
    let v = child.finish(130);
    assert_eq!(v["steps"][0]["status"], "succeeded");
    assert_eq!(v["steps"][1]["status"], "cancelled");
    assert_eq!(v["steps"][2]["status"], "cancelled");
    settled(&f, &v);
}

#[test]
fn pre_spawn_refusals_keep_their_status_and_cannot_mimic_cancellation() {
    let f = Fixture::new("parallel");
    f.edit(|c| c["max_active"] = json!(1));
    f.json(
        "input.json",
        json!([input("a", "barrier", 0), input("b", "approve", 0)]),
    );
    let mut child = f.start("run");
    child.ready("a/ready");
    child.ready("artifacts/job-001/admitted.json");
    fs::rename(f.root.join("b"), f.root.join("original-b")).unwrap();
    symlink(f.root.join("c"), f.root.join("b")).unwrap();
    f.write("a/release", "go");
    let v = child.finish(0);
    assert_eq!(v["steps"][1]["exit"]["started"], false);
    assert_eq!(v["steps"][1]["status"], "not_started");
    assert!(
        v["steps"][1]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains("workspace changed"))
    );
    assert!(f.calls("c").is_empty());
    settled(&f, &v);

    // Caller paths are data, never a signal/cancellation classification.
    let f = Fixture::new("reviewer");
    f.write(
        "cancelled-not-executable",
        "invalid executable without a shebang",
    );
    fs::set_permissions(
        f.root.join("cancelled-not-executable"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    f.edit(|c| c["due_binary"] = json!("cancelled-not-executable"));
    let v = f.run(0);
    assert_eq!(v["steps"][0]["status"], "not_started");
    assert_eq!(v["steps"][0]["stopped_before_spawn"], false);
    assert_eq!(v["steps"][0]["exit"]["started"], false);
    assert!(f.calls("a").is_empty());
    settled(&f, &v);
}

#[test]
fn ordinary_stage_replacement_uses_code_without_acquiring_more_authority() {
    let f = Fixture::new("pure");
    f.script("(local cycle (require :cycle))\n(local r (require :reviewer))\n{:meta meta :run (fn [input] (cycle.with-stages input r.run (fn [context] (assert (= (data.kind (data.get context :findings)) :array)) (data.pack {:ok false :status :replacement_declined}))))}");
    f.json("input.json", input("a", "endless", 2));
    let v = f.run(0);
    assert_eq!(status(&v), "replacement_declined");
    assert_eq!(f.calls("a").len(), 1);
    let f = Fixture::new("pure");
    f.script("(local cycle (require :cycle))\n(local r (require :reviewer))\n{:meta meta :run (fn [input] (cycle.with-stages input r.run (fn [context] (host.run {:role :ungranted :workspace :a :input context :prompt :never}))))}");
    f.json("input.json", input("a", "endless", 2));
    let v = f.run(1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown caller-granted role")
    );
    assert_eq!(f.calls("a").len(), 1);
}

#[test]
fn library_cancellation_and_dropping_future_join_owned_children() {
    // Deadline-supervise even the library Drop probe in an isolated test process.
    const PROBE: &str = "FINOCCHIO_LIBRARY_PROBE";
    if std::env::var_os(PROBE).is_some() {
        use finocchio_spike::workflow::{Cancellation, Config, Mode, execute};
        for drop_future in [false, true] {
            let f = Fixture::new("parallel");
            let input = json!([input("a", "barrier", 0), input("b", "barrier", 0)]);
            let config = Config::load(&f.root.join("config.json")).unwrap();
            let token = Cancellation::default();
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
                let artifacts=f.root.join("artifacts");let mut future=Box::pin(execute(config,Mode::Run,input,Some(&artifacts),token.clone()));
                let deadline=tokio::time::Instant::now()+Duration::from_secs(10);
                loop {
                    tokio::select! {
                        _ = &mut future => panic!("library exited before barriers"),
                        _ = tokio::time::sleep(Duration::from_millis(5)) => if f.root.join("a/ready").exists() && f.root.join("b/ready").exists() {break;},
                        _ = tokio::time::sleep_until(deadline) => panic!("library barrier deadline"),
                    }
                }
                if drop_future {drop(future);} else {token.cancel();assert_eq!(future.await["status"],"cancelled");}
            });
            assert!(f.root.join("a/cleaned").exists());
            assert!(f.root.join("b/cleaned").exists());
            let v = f.read("artifacts/workflow.json");
            assert_eq!(v["status"], "cancelled");
            settled(&f, &v);
        }
        // Use the public token so later cancellation is synchronously observed
        // before releasing cleanup; OS signal scheduling cannot weaken this gate.
        let f = Fixture::new("pure");
        f.script("(local r (require :reviewer))\n{:meta meta :run (fn [input] (host.concurrent [(fn [] (r.run (data.get input 1))) (fn [] (r.run (data.get input 2)) (error :first-failure))]))}");
        let config = Config::load(&f.root.join("config.json")).unwrap();
        let input = json!([input("a", "slow-cancel", 0), input("b", "barrier", 0)]);
        let token = Cancellation::default();
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let artifacts = f.root.join("artifacts");
            let mut future = Box::pin(execute(config, Mode::Run, input, Some(&artifacts), token.clone()));
            for marker in ["a/ready", "b/ready", "a/cancel-seen"] {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                loop {
                    tokio::select! {
                        _ = &mut future => panic!("exited before {marker}"),
                        _ = tokio::time::sleep(Duration::from_millis(5)) => if f.root.join(marker).exists() {break;},
                        _ = tokio::time::sleep_until(deadline) => panic!("barrier deadline {marker}"),
                    }
                }
                if marker == "b/ready" { f.write("b/release", "go"); }
            }
            token.cancel();
            assert!(token.is_cancelled());
            f.write("a/cleanup-release", "go");
            let v = future.await;
            assert_eq!(v["status"], "failed");
            assert_eq!(v["cancellation_requested"], true);
            assert!(v["error"]["message"].as_str().unwrap().contains("first-failure"));
            settled(&f, &v);
        });
        return;
    }
    let f = Fixture::new("pure");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library_cancellation_and_dropping_future_join_owned_children",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .stdout(fs::File::create(f.root.join("stdout")).unwrap())
        .stderr(fs::File::create(f.root.join("stderr")).unwrap())
        .spawn()
        .unwrap();
    let mut guard = Running {
        child: Some(child),
        root: f.root.clone(),
    };
    let until = Instant::now() + Duration::from_secs(20);
    while guard.child.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < until, "library probe deadline");
        thread::sleep(Duration::from_millis(10));
    }
    let result = guard.child.take().unwrap().wait().unwrap();
    assert!(
        result.success(),
        "{}",
        fs::read_to_string(f.root.join("stdout")).unwrap()
    );
}

#[test]
fn early_exit_during_request_intake_preserves_failure_and_does_not_cancel_sibling() {
    let f = Fixture::new("pure");
    let prologue = "#!/usr/bin/env python3\nimport sys, json\nfrom pathlib import Path\nif Path(sys.argv[7]).parent.name == 'job-000':\n    Path(sys.argv[7]).write_text('{}\\n')\n    print(json.dumps({'schema_version':1,'provider':'codex','text':'','answer_schema_validation':'not_performed','duration_ms':0,'status':'failed','error':{'kind':'configuration','message':'synthetic early intake failure'},'structured_output_present':False,'structured_output':None}),flush=True)\n    sys.exit(1)\n";
    f.write(
        "due",
        &(prologue.to_string() + include_str!("fixtures/due.py")),
    );
    f.script("(local r (require :reviewer))\n{:meta meta :run (fn [input] (host.concurrent [(fn [] (host.run {:role :reviewer :workspace :a :input (data.get input 1) :prompt (string.rep :x 120000)})) (fn [] (r.run (data.get input 2)))]))}");
    f.json(
        "input.json",
        json!([input("a", "approve", 0), input("b", "approve", 0)]),
    );
    let v = f.run(0);
    assert_eq!(v["steps"][0]["status"], "native_failure");
    assert_eq!(
        v["steps"][0]["child_error"]["message"],
        "synthetic early intake failure"
    );
    assert_eq!(v["steps"][1]["status"], "succeeded");
    settled(&f, &v);
}

#[test]
fn zero_arity_contracts_describe_and_zero_job_limits_reject_admission() {
    let f = Fixture::new("pure");
    f.script("(import-macros {: defn} :finocchio)\n(defn helper [] \"A zero-arity pure helper.\" true)\n{:meta meta :helper helper :run (fn [input] (helper))}");
    let v = f.start("describe").finish(0);
    let docs = v["output"]["probe"]["functions"].as_array().unwrap();
    let helper = docs.iter().find(|v| v["name"] == "helper").unwrap();
    assert_eq!(helper["contract"]["parameters"], json!([]));
    assert_eq!(helper["contract"]["arity"], 0);
    let f = Fixture::new("reviewer");
    f.edit(|c| c["max_jobs"] = json!(0));
    let v = f.run(1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("total job cap")
    );
    assert!(f.calls("a").is_empty());
}

#[test]
fn prepared_live_pair_is_checked_only_with_synthetic_children() {
    let f = Fixture::new("pure");
    f.write(
        "pair.fnl",
        include_str!("../examples/workflow/live-pair.fnl"),
    );
    f.edit(|c| {
        c["modules"]["pair"] = json!("pair.fnl");
        c["entry"] = json!("pair");
        c["max_jobs"] = json!(2);
        c["max_active"] = json!(1);
    });
    f.json(
        "input.json",
        json!({"workspace":"a","task":"approve","findings":["synthetic seeded finding"]}),
    );
    let v = f.run(0);
    assert_eq!(status(&v), "approved");
    let calls = f.calls("a");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["revision"], true);
    assert_eq!(calls[1]["revision"], false);
    settled(&f, &v);
}
#[test]
fn script_error_joins_sibling_and_retains_completed_receipts() {
    let f = Fixture::new("pure");
    f.script("(local r (require :reviewer))\n{:meta meta :run (fn [input] (host.concurrent [(fn [] (r.run (data.get input 1))) (fn [] (r.run (data.get input 2)) (error :deliberate))]))}");
    f.json(
        "input.json",
        json!([input("a", "barrier", 1), input("b", "barrier", 1)]),
    );
    let mut child = f.start("run");
    child.ready("a/ready");
    child.ready("b/ready");
    f.write("b/release", "go");
    let v = child.finish(1);
    assert_eq!(v["steps"][1]["status"], "succeeded");
    assert_eq!(v["steps"][0]["status"], "cancelled");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("probe.fnl")
    );
    settled(&f, &v);
}
#[test]
fn captures_are_bounded_and_exited_due_with_pipe_writers_keeps_native_failure() {
    for task in ["flood", "trace-flood"] {
        let f = Fixture::new("reviewer");
        f.json("input.json", input("a", task, 1));
        let v = f.run(1);
        stopped(&v);
        for file in ["result.json", "stderr.txt", "trace.jsonl"] {
            assert!(
                fs::metadata(f.root.join("artifacts/job-000").join(file))
                    .unwrap()
                    .len()
                    <= 1024 * 1024
            );
        }
        settled(&f, &v);
    }
    let f = Fixture::new("reviewer");
    f.json("input.json", input("a", "pipe-writer", 1));
    let v = f.run(0);
    assert_eq!(v["output"]["status"], "native_failure");
    assert!(
        v["output"]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains("pipe still open"))
    );
    f.write("a/orphan-release", "go");
    settled(&f, &v);
}
#[test]
fn artifacts_are_exclusive_and_module_resolution_is_explicit() {
    let f = Fixture::new("pure");
    fs::create_dir(f.root.join("artifacts")).unwrap();
    f.write("artifacts/sentinel", "untouched");
    stopped(&f.run(1));
    assert_eq!(
        fs::read_to_string(f.root.join("artifacts/sentinel")).unwrap(),
        "untouched"
    );
    let f = Fixture::new("pure");
    f.script("(require :not-configured)\n{:meta meta :run (fn [input] input)}");
    let v = f.run(1);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("module not configured")
    );
}

#[test]
fn caller_codex_permission_configuration_survives_profile_selection() {
    let f = Fixture::new("reviewer");
    let native = json!([
        "-c",
        "sandbox_mode=\"read-only\"",
        "-c",
        "approval_policy=\"never\"",
        "-c",
        "agents.enabled=false",
        "-c",
        "features.multi_agent=false",
        "-c",
        "features.multi_agent_v2=false",
        "--config",
        "sandbox_workspace_write.network_access=false",
        "-c",
        "sandbox_workspace_write.writable_roots=[]",
        "-c",
        "sandbox_workspace_write.exclude_slash_tmp=true",
        "-c",
        "sandbox_workspace_write.exclude_tmpdir_env_var=true"
    ]);
    f.edit(|c| c["profiles"]["review"]["native_args"] = native.clone());
    let v = f.run(0);
    assert_eq!(v["output"]["status"], "succeeded");
    assert_eq!(f.calls("a")[0]["request"]["native_args"], native);
    assert_eq!(
        v["steps"][0]["admission"]["selected_settings"]["native_args"],
        native
    );
    settled(&f, &v);
}

#[test]
fn two_complete_revision_cycles_overlap_and_keep_input_order() {
    let f = Fixture::new("parallel");
    f.edit(|c| c["max_jobs"] = json!(6));
    f.json(
        "input.json",
        json!([
            input("a", "barrier-revise", 1),
            input("b", "barrier-revise", 1)
        ]),
    );
    let mut child = f.start("run");
    child.ready("a/ready");
    child.ready("b/ready");
    f.write("b/release", "go");
    child.ready("artifacts/job-003/receipt.json");
    assert!(!f.root.join("artifacts/job-000/receipt.json").exists());
    f.write("a/release", "go");
    let v = child.finish(0);
    assert_eq!(v["output"]["approved"], 2);
    assert_eq!(v["steps"].as_array().unwrap().len(), 6);
    assert_eq!(v["output"]["results"][0]["last"]["id"], 5);
    assert_eq!(v["output"]["results"][1]["last"]["id"], 3);
    for workspace in ["a", "b"] {
        let calls = f.calls(workspace);
        assert_eq!(
            calls
                .iter()
                .map(|c| c["revision"].clone())
                .collect::<Vec<_>>(),
            vec![json!(false), json!(true), json!(false)]
        );
        assert!(f.root.join(workspace).join("mutation").exists());
    }
    for result in v["output"]["results"].as_array().unwrap() {
        assert_eq!(result["revisions"], 1);
        assert_eq!(result["status"], "approved");
    }
    settled(&f, &v);
}

#[test]
fn first_stop_reason_survives_later_cancellation_or_capture_failure() {
    let f = Fixture::new("pure");
    f.script("(local r (require :reviewer))\n{:meta meta :run (fn [input] (host.concurrent [(fn [] (r.run (data.get input 1))) (fn [] (r.run (data.get input 2)) (error :first-failure))]))}");
    f.json(
        "input.json",
        json!([input("a", "slow-cancel", 0), input("b", "barrier", 0)]),
    );
    let mut child = f.start("run");
    child.ready("a/ready");
    child.ready("b/ready");
    f.write("b/release", "go");
    child.ready("a/cancel-seen"); // Fatal script error has already signalled the child.
    // The signal may race final cleanup. This CLI case only checks fatal-first
    // status retention; the supervised library token case above proves that
    // cancellation was actually recorded before cleanup was released.
    child.signal(libc::SIGINT);
    f.write("a/cleanup-release", "go");
    let v = child.finish(1);
    assert_eq!(v["status"], "failed");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("first-failure")
    );
    settled(&f, &v);

    let f = Fixture::new("reviewer");
    f.json("input.json", input("a", "cancel-flood", 0));
    let mut child = f.start("run");
    child.ready("a/ready");
    child.signal(libc::SIGTERM); // Overflow occurs only after the child receives this stop.
    let v = child.finish(130);
    assert_eq!(v["status"], "cancelled");
    assert_eq!(v["error"]["message"], "parent cancelled");
    assert!(
        v["steps"][0]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains("exceeded"))
    );
    settled(&f, &v);
}

#[test]
fn continuation_depth_active_and_total_limits_are_shared() {
    for body in [
        "(fn deep [n] (if (= n 0) true (host.concurrent [(fn [] (deep (- n 1)))]))) {:meta meta :run (fn [input] (deep 9))}",
        // Only 65 total continuations at depth 3: disabling the active cap
        // must make this succeed, rather than falling through to the total cap.
        "(fn wide [first] (let [fs [first]] (for [i 2 32] (table.insert fs (fn [] true))) fs)) {:meta meta :run (fn [input] (host.concurrent (wide (fn [] (host.concurrent (wide (fn [] (host.concurrent [(fn [] true)]))))))))}",
        "{:meta meta :run (fn [input] (for [i 1 257] (host.concurrent [(fn [] true)])) true)}",
    ] {
        let f = Fixture::new("pure");
        f.script(body);
        let v = f.run(1);
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("continuation budget"),
            "{v}"
        );
        assert_eq!(v["steps"], json!([]));
    }
}
