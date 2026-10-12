#![allow(dead_code)]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

pub struct Fixture {
    pub root: TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if thread::panicking() {
            self.root.disable_cleanup(true);
            eprintln!(
                "retained failing workflow fixture: {}",
                self.root.path().display()
            );
        }
    }
}
impl Fixture {
    pub fn new(script: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let f = Self { root };
        f.write("agent", include_str!("../fixtures/workflow-agent.sh"));
        fs::set_permissions(
            f.root.path().join("agent"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        f.write("main.rhai", script);
        f.json("input.json", json!({"task":"synthetic review"}));
        f.json("input.schema.json", json!({"type":"object","required":["task"],"properties":{"task":{"type":"string"}},"additionalProperties":false}));
        f.json("answer.schema.json", json!({"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"},"needs_more_evidence":{"type":"boolean"},"findings":{"type":"array","items":{"type":"string"}}},"additionalProperties":false}));
        f.json("output.schema.json", json!(true));
        f.json("workflow.json", json!({
            "schema_version":1,"entry":"main.rhai","input_schema":"input.schema.json","output_schema":"output.schema.json","max_runs":8,"max_concurrency":2,
            "profiles":{"reader":{"binary":"agent","request":{"schema_version":1,"provider":"claude","cwd":".","schema":"answer.schema.json","native_args":["--permission-mode","plan"]}}},"modules":{}
        }));
        f
    }
    pub fn write(&self, name: &str, text: &str) {
        fs::write(self.root.path().join(name), text).unwrap();
    }
    pub fn json(&self, name: &str, v: Value) {
        self.write(name, &v.to_string());
    }
    pub fn read(&self, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.root.path().join(name)).unwrap()).unwrap()
    }
    pub fn edit(&self, edit: impl FnOnce(&mut Value)) {
        let mut m = self.read("workflow.json");
        edit(&mut m);
        self.json("workflow.json", m);
    }
    pub fn command(&self, check: bool) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_due"));
        c.args(["workflow", if check { "check" } else { "run" }])
            .arg(self.root.path().join("workflow.json"))
            .args(["--input", "input.json", "--output", "json"])
            .env("DUE_WORKFLOW_TEST_DIR", self.root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !check {
            c.arg("--artifacts").arg(self.root.path().join("artifacts"));
        }
        c
    }
    pub fn run(&self, check: bool, code: i32) -> Value {
        envelope(finish(self.command(check).spawn().unwrap()), code)
    }
    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.path().join("prompts"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}
pub fn finish(mut c: Child) -> Output {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        if c.try_wait().unwrap().is_some() {
            return c.wait_with_output().unwrap();
        }
        if Instant::now() >= until {
            let _ = c.kill();
            let output = c.wait_with_output().unwrap();
            panic!("deadline: {output:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
pub fn envelope(output: Output, code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["workflow_version"], 1);
    v
}
pub fn wait_for(path: &Path, child: &mut Child) {
    let until = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(child.try_wait().unwrap().is_none(), "exited before ready");
        if Instant::now() >= until {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            // Signal cleanup rather than abandoning real test children on failure.
            let cleanup_until = Instant::now() + Duration::from_secs(10);
            while child.try_wait().unwrap().is_none() && Instant::now() < cleanup_until {
                thread::sleep(Duration::from_millis(10));
            }
            panic!("never ready: {}", path.display());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Reaping may briefly lag wrapper completion; zombies cannot execute work.
pub fn assert_native_cleanup(f: &Fixture) {
    for pid in fs::read_to_string(f.root.path().join("pids"))
        .unwrap_or_default()
        .lines()
    {
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
            assert!(
                Instant::now() < until,
                "native process {pid} survived cleanup"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
