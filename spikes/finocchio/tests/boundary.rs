use finocchio_spike::{compiler, runtime};
use std::{
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn compiler_and_runtime_capabilities_are_separate_and_no_ambient_loader_exists() {
    let source = r#"(macro probe []
      (assert (= (. _G :io) nil))
      (assert (= (. _G :os) nil))
      (assert (= (. _G :debug) nil))
      (assert (= (. _G :package) nil))
      (assert (= (. _G :loadfile) nil))
      (assert (= (. _G :load) nil))
      (assert (= (. _G :require) nil))
      (assert (= (. _G :due) nil))
      (assert (= (. _G :contract) nil))
      (assert (= (. _G :data) nil))
      42)
      (probe)"#;
    let code = compiler::compile(source, "capabilities.fnl").unwrap();
    let lua = runtime().unwrap();
    assert_eq!(lua.load(code).eval::<i64>().unwrap(), 42);
    lua.load("assert(_VERSION=='Lua 5.4'); assert(io==nil and os==nil and debug==nil and package==nil and load==nil and loadfile==nil and dofile==nil and require==nil and due==nil)").exec().unwrap();
    for source in [
        "(import-macros m :ambient)",
        "(include :ambient)",
        "(eval-compiler (require :ambient))",
    ] {
        assert!(compiler::compile(source, "denied.fnl").is_err(), "{source}");
    }
}

#[test]
fn compiler_lua_loop_exhaustion_is_sticky() {
    const CASES: &[&str] = &[
        "(macro spin [] (while true)) (spin)",
        "(macro spin [] (while true (pcall (fn [] (while true nil))))) (spin)",
        "(macro spin [] (while true (pcall (fn [] (while true (pcall (fn [] (while true nil)))))))) (spin)",
        "(macro spin [] (let [catch _G.pcall] (while true (catch (fn [] (while true nil)))))) (spin)",
        "(macro spin [] (let [catch (. (. (getmetatable _G) :__index) :pcall)] (while true (catch (fn [] (while true nil)))))) (spin)",
    ];
    if let Ok(case) = std::env::var("FINOCCHIO_BUDGET_CASE") {
        let compiler = compiler::Compiler::new().unwrap();
        let source = CASES[case.parse::<usize>().unwrap() % CASES.len()];
        let result = if case.parse::<usize>().unwrap() < CASES.len() {
            compiler.compile(source, "loop.fnl").map(|_| ())
        } else {
            compiler.boundary_probe(source, "loop.fnl").map(|_| ())
        };
        let error = result.unwrap_err();
        assert!(error.to_string().contains("budget exhausted"), "{error}");
        let error = compiler.compile("42", "after.fnl").unwrap_err();
        assert!(error.to_string().contains("budget exhausted"), "{error}");
        return;
    }
    // Own only this compiler test child. Drop kills AND reaps even on failure.
    struct OwnedChild(Option<Child>);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    // Retain every original case; run the identical cases through the opt-in
    // driver too, with the same containment guard and sticky-exhaustion check.
    for case in 0..2 * CASES.len() {
        let mut child = OwnedChild(Some(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "compiler_lua_loop_exhaustion_is_sticky",
                    "--nocapture",
                ])
                .env("FINOCCHIO_BUDGET_CASE", case.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        ));
        let until = Instant::now() + Duration::from_secs(5);
        while child.0.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < until,
                "compiler budget case {case} hung for 5s"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let output = child.0.take().unwrap().wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "case {case}: {stdout}");
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "case {case} ran no test: {stdout}"
        );
    }
}

#[test]
fn compiler_protected_calls_still_catch_ordinary_errors_and_preserve_nil_slots() {
    let source = r#"(macro probe []
      (let [(ok a b c) (pcall (fn [] (values 1 nil 3)))
            (caught message) (pcall (fn [] (error :ordinary)))]
        (assert (and ok (= a 1) (= b nil) (= c 3)))
        (assert (not caught))
        (assert (string.find message :ordinary))
        42))
      (probe)"#;
    let code = compiler::compile(source, "protected.fnl").unwrap();
    assert_eq!(runtime().unwrap().load(code).eval::<i64>().unwrap(), 42);
}
