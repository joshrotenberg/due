#![allow(dead_code)]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

pub struct Fixture {
    pub root: TempDir,
}

/// Own the workflow through assertions too, with file captures that cannot fill
/// an unread pipe. Keep the readiness guard at ten seconds, including JSON writes.
pub struct Running {
    child: Option<Child>,
    root: PathBuf,
}
impl Running {
    pub fn new(f: &Fixture) -> Self {
        Self::with_command(f, f.command(false))
    }
    pub fn with_command(f: &Fixture, mut command: Command) -> Self {
        command.stdout(fs::File::create(f.root.path().join("test-stdout")).unwrap());
        command.stderr(fs::File::create(f.root.path().join("test-stderr")).unwrap());
        Self {
            child: Some(command.spawn().unwrap()),
            root: f.root.path().into(),
        }
    }
    pub fn ready(&mut self, name: &str) {
        self.until(name, |path| path.exists().then_some(()));
    }
    pub fn receipt(&mut self, name: &str) -> Value {
        // save() creates its destination before serializing. Existence alone
        // does not mean a receipt is complete; the newline is its final write.
        self.until(name, |path| {
            let bytes = fs::read(path).ok()?;
            if bytes.last() != Some(&b'\n') {
                return None;
            }
            Some(serde_json::from_slice(&bytes).expect("complete JSON receipt"))
        })
    }
    fn until<T>(&mut self, name: &str, read: impl Fn(&Path) -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(value) = read(&self.root.join(name)) {
                return value;
            }
            assert!(self.alive(), "workflow exited before {name}");
            assert!(Instant::now() < until, "never ready: {name}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn alive(&mut self) -> bool {
        self.child.as_mut().unwrap().try_wait().unwrap().is_none()
    }
    pub fn finish(mut self, code: i32) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        while self.alive() {
            assert!(Instant::now() < until, "workflow fixture deadline");
            thread::sleep(Duration::from_millis(10));
        }
        let mut output = self.child.take().unwrap().wait_with_output().unwrap();
        output.stdout = fs::read(self.root.join("test-stdout")).unwrap();
        output.stderr = fs::read(self.root.join("test-stderr")).unwrap();
        envelope(output, code)
    }
}

#[cfg(target_os = "macos")]
pub fn inject_pipe_window(f: &Fixture, command: &mut Command) {
    let library = f.root.path().join("pipe-window.dylib");
    f.write(
        "pipe-window.c",
        include_str!("../fixtures/workflow-pipe-window.c"),
    );
    let compiler = Command::new("/usr/bin/clang")
        .args(["-std=c11", "-dynamiclib", "-Wall", "-Wextra", "-Werror"])
        .arg(f.root.path().join("pipe-window.c"))
        .arg("-o")
        .arg(&library)
        .stdin(Stdio::null())
        .stdout(fs::File::create(f.root.path().join("compiler-stdout")).unwrap())
        .stderr(fs::File::create(f.root.path().join("compiler-stderr")).unwrap())
        .status()
        .unwrap();
    assert!(
        compiler.success(),
        "pipe interposer compilation: {}",
        fs::read_to_string(f.root.path().join("compiler-stderr")).unwrap()
    );
    command
        .env("DYLD_INSERT_LIBRARIES", library)
        .env("DUE_PIPE_WINDOW_LOG", f.root.path().join("pipe-window.log"))
        .env("TOKIO_WORKER_THREADS", "4");
}
impl Drop for Running {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // Snapshot before TERM changes receipts/traces. Fixture retains this
        // directory on unwind; cleanup output stays alongside the snapshot.
        fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
            fs::create_dir(to)?;
            for entry in fs::read_dir(from)? {
                let entry = entry?;
                let target = to.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    copy_tree(&entry.path(), &target)?;
                } else {
                    fs::copy(entry.path(), target)?;
                }
            }
            Ok(())
        }
        let snapshot = copy_tree(
            &self.root.join("artifacts"),
            &self.root.join("before-cleanup"),
        );
        let _ = fs::write(
            self.root.join("test-cleanup"),
            format!("workflow_pid={} snapshot={snapshot:?}\n", child.id()),
        );
        if matches!(child.try_wait(), Ok(None)) {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let until = Instant::now() + Duration::from_secs(10);
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < until {
                thread::sleep(Duration::from_millis(10));
            }
            if matches!(child.try_wait(), Ok(None)) {
                // Emergency cleanup uses only this synthetic fixture's recorded
                // native identities, then lets Due reap them before killing Due.
                for pid in fs::read_to_string(self.root.join("pids"))
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|s| s.parse::<i32>().ok())
                {
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
                let until = Instant::now() + Duration::from_secs(3);
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < until {
                    thread::sleep(Duration::from_millis(10));
                }
                if matches!(child.try_wait(), Ok(None)) {
                    // A stuck Due supervisor must not outlive a failed test.
                    for entry in fs::read_dir(self.root.join("artifacts/steps"))
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        if entry.path().join("exit.json").exists() {
                            continue;
                        }
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
            }
        }
        let joined = child.wait();
        let _ = fs::OpenOptions::new()
            .append(true)
            .open(self.root.join("test-cleanup"))
            .and_then(|mut file| {
                use std::io::Write;
                writeln!(file, "joined={joined:?}")
            });
    }
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
