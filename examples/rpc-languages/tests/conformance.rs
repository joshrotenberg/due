#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn languages() -> Vec<&'static str> {
    vec![
        #[cfg(feature = "js")]
        "js",
        #[cfg(feature = "lua")]
        "lua",
    ]
}

fn script(language: &str, case: &str) -> String {
    let body = match (language, case) {
        ("js", "conversation" | "shapes" | "mutation") => {
            "const a=c.turn('remember marker-731'); check(a.status==='succeeded','first turn');
             const b=c.turn('recall'); check(b.text==='marker-731','second turn');
             check(native===2,'all native items'); c.close();"
        }
        ("lua", "conversation" | "shapes" | "mutation") => {
            "local a=c:turn('remember marker-731'); check(a.status=='succeeded','first turn')
             local b=c:turn('recall'); check(b.text=='marker-731','second turn')
             check(native==2,'all native items'); c:close()"
        }
        ("js", "out_of_order") => {
            "const a=c.request('test.echo',{value:1}),b=c.request('test.echo',{value:2});
             check(c.await(a).value===1,'first response'); check(c.await(b).value===2,'second response'); c.close();"
        }
        ("lua", "out_of_order") => {
            "local a=c:request('test.echo',{value=1}); local b=c:request('test.echo',{value=2})
             check(c:await(a).value==1,'first response'); check(c:await(b).value==2,'second response'); c:close()"
        }
        ("js", "rpc_error") => {
            "let caught=false; try { c.await(c.request('test.error',{})); }
             catch(e) { check(e.code===-32602 && e.data.field==='prompt','structured RPC error'); caught=true; }
             check(caught,'RPC error must throw'); check(c.turn('next').status==='succeeded','reuse after RPC error'); c.close();"
        }
        ("lua", "rpc_error") => {
            "local ok,e=pcall(function() c:await(c:request('test.error',{})) end)
             check(not ok and e.code==-32602 and e.data.field=='prompt','structured RPC error')
             check(c:turn('next').status=='succeeded','reuse after RPC error'); c:close()"
        }
        ("js", "failed_turn") => {
            "check(c.turn('fail').status==='failed','failed result is data');
             check(c.turn('next').status==='succeeded','reuse after failed turn'); c.close();"
        }
        ("lua", "failed_turn") => {
            "check(c:turn('fail').status=='failed','failed result is data')
             check(c:turn('next').status=='succeeded','reuse after failed turn'); c:close()"
        }
        ("js", "reply_on_item") => {
            "check(c.turn('permission').status==='succeeded','permission turn');
             check(interactions===0,'already answered handler suppressed'); c.await(manualReply); c.close();"
        }
        ("lua", "reply_on_item") => {
            "check(c:turn('permission').status=='succeeded','permission turn')
             check(interactions==0,'already answered handler suppressed'); c:await(manual_reply); c:close()"
        }
        ("js", "retired_reply") => {
            "check(c.turn('permission').status==='succeeded','permission turn');
             check(blockedReplies===2,'both retired replies rejected'); c.close();"
        }
        ("lua", "retired_reply") => {
            "check(c:turn('permission').status=='succeeded','permission turn')
             check(blocked_replies==2,'both retired replies rejected'); c:close()"
        }
        ("js", "permission") => {
            "check(c.turn('permission').status==='succeeded','permission turn');
             check(interactions===1,'one permission callback'); c.close();"
        }
        ("lua", "permission") => {
            "check(c:turn('permission').status=='succeeded','permission turn')
             check(interactions==1,'one permission callback'); c:close()"
        }
        ("js", "interrupt") => {
            "const t=c.start('permission'); check(deferred!==null,'deferred permission');
             const id=c.interrupt(t); check(c.wait(t).status==='failed','interrupted turn'); c.await(id);
             let blocked=false; try { c.reply(deferred,{decision:'decline'}); } catch(e) { blocked=true; }
             check(blocked,'stale permission rejected'); check(c.turn('next').status==='succeeded','reuse after interrupt'); c.close();"
        }
        ("lua", "interrupt") => {
            "local t=c:start('permission'); check(deferred~=nil,'deferred permission')
             local id=c:interrupt(t); check(c:wait(t).status=='failed','interrupted turn'); c:await(id)
             local ok=pcall(function() c:reply(deferred,{decision='decline'}) end)
             check(not ok,'stale permission rejected'); check(c:turn('next').status=='succeeded','reuse after interrupt'); c:close()"
        }
        ("js", "sticky_session" | "sticky_callback") => {
            "let first=null,second=null;
             try { if(testCase==='sticky_callback') c.turn('trigger'); else c.poll(1000); } catch(e) { first=e; }
             check(first!==null,'initial failure required');
             try { c.request('test.echo',{}); } catch(e) { second=e; }
             check(first===second,'first failure must remain sticky');"
        }
        ("lua", "sticky_session" | "sticky_callback") => {
            "local ok,first=pcall(function()
               if test_case=='sticky_callback' then c:turn('trigger') else c:poll(1000) end
             end)
             check(not ok,'initial failure required')
             local ok2,second=pcall(function() c:request('test.echo',{}) end)
             check(not ok2 and first==second,'first failure must remain sticky')"
        }
        ("js", "unsafe_outbound") => {
            "c.request('test.echo',{value:9007199254740992}); throw new Error('expected failure was not raised');"
        }
        ("js", "discarded_rejection") => {
            "Promise.reject(new Error('fixture discarded rejection')); void 0;"
        }
        ("js", "session_failed" | "gap" | "omitted") => {
            "for(let i=0;i<10;i++) c.poll(100); throw new Error('expected failure was not raised');"
        }
        ("lua", "session_failed" | "gap" | "omitted") => {
            "for i=1,10 do c:poll(100) end; error('expected failure was not raised')"
        }
        ("js", "callback" | "permission_missing") => {
            "c.turn('trigger'); throw new Error('expected failure was not raised');"
        }
        ("lua", "callback" | "permission_missing") => {
            "c:turn('trigger'); error('expected failure was not raised')"
        }
        // These fail while opening, so this statement must not be reached.
        ("js", "bad_json" | "bad_utf8" | "reentrant" | "async_callback" | "invalid_id" | "invalid_version") => {
            "throw new Error('expected failure was not raised');"
        }
        ("lua", "bad_json" | "bad_utf8" | "reentrant" | "async_callback" | "invalid_id" | "invalid_version") => {
            "error('expected failure was not raised')"
        }
        _ => panic!("missing script for {language}/{case}"),
    };
    if language == "js" {
        format!(
            r#"
const testCase={case:?};
function check(value,message) {{ if(!value) throw new Error(message); }}
let native=0,interactions=0,deferred=null,c,manualReply,held,blockedReplies=0;
c=new DueClient({{
  onItem(item) {{
    bridge.log('ITEM:'+item.kind);
    if(item.kind==='interaction') {{ held=item; if(testCase==='reply_on_item') manualReply=c.reply(item,{{decision:'decline'}}); }}
    if(testCase==='retired_reply' && (item.kind==='interaction_resolved' || item.kind==='turn_finished')) {{
      let blocked=false; try {{ c.reply(held,{{decision:'decline'}}); }} catch(e) {{ blocked=true; }}
      check(blocked,'reply during terminal callback must be rejected'); blockedReplies++;
    }}
    if(testCase==='mutation' && item.kind==='turn_finished') {{ item.data.status='failed'; item.data.text='tampered'; item.data.run_id='forged'; }}
    if(testCase==='reentrant') c.poll(0);
    if(testCase==='async_callback') return Promise.resolve();
    if(item.kind==='native') {{
      native++;
      if(testCase==='callback' || testCase==='sticky_callback') throw new Error('fixture observer failed');
      if(testCase==='shapes') {{
        check(item.data.null===null,'JSON null');
        check(Array.isArray(item.data.array) && item.data.array.length===0,'empty array');
        check(!Array.isArray(item.data.object) && Object.keys(item.data.object).length===0,'empty object');
        check(item.data.large===9007199254740992,'documented Number view');
      }}
    }}
  }},
  onInteraction: testCase==='permission_missing' ? undefined : function(item) {{
    interactions++;
    if(testCase==='interrupt') {{ deferred=item; return null; }}
    return {{decision:'decline'}};
  }}
}});
c.open({{provider:'codex'}});
{body}
if(testCase!=='discarded_rejection') bridge.log('WORKFLOW_OK');
"#
        )
    } else {
        format!(
            r#"
local test_case={case:?}
local function check(value,message) if not value then error(message) end end
local native,interactions,deferred,c=0,0,nil,nil
local manual_reply,held,blocked_replies=nil,nil,0
local options={{
  onItem=function(item)
    bridge.log('ITEM:'..item.kind)
    if item.kind=='interaction' then held=item; if test_case=='reply_on_item' then manual_reply=c:reply(item,{{decision='decline'}}) end end
    if test_case=='retired_reply' and (item.kind=='interaction_resolved' or item.kind=='turn_finished') then
      local ok=pcall(function() c:reply(held,{{decision='decline'}}) end)
      check(not ok,'reply during terminal callback must be rejected'); blocked_replies=blocked_replies+1
    end
    if test_case=='mutation' and item.kind=='turn_finished' then item.data.status='failed'; item.data.text='tampered'; item.data.run_id='forged' end
    if test_case=='reentrant' then c:poll(0) end
    if test_case=='async_callback' then coroutine.yield() end
    if item.kind=='native' then
      native=native+1
      if test_case=='callback' or test_case=='sticky_callback' then error('fixture observer failed') end
      if test_case=='shapes' then
        check(item.data.null==json.null,'JSON null')
        check(json.encode(item.data.array)=='[]','empty array')
        check(json.encode(item.data.object)=='{{}}','empty object')
        check(item.data.large==9007199254740993,'signed integer view')
      end
    end
  end,
  onInteraction=function(item)
    interactions=interactions+1
    if test_case=='interrupt' then deferred=item; return nil end
    return {{decision='decline'}}
  end
}}
if test_case=='permission_missing' then options.onInteraction=nil end
c=DueClient.new(options)
c:open({{provider='codex'}})
{body}
bridge.log('WORKFLOW_OK')
"#
        )
    }
}

fn run_case(language: &str, case: &str, success: bool) {
    let dir = tempfile::tempdir().unwrap();
    let peer = dir.path().join("peer");
    fs::write(&peer, include_bytes!("peer.py")).unwrap();
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o755)).unwrap();
    let workflow = dir.path().join(format!("workflow.{language}"));
    fs::write(&workflow, script(language, case)).unwrap();
    let evidence = dir.path().join("evidence");
    let output = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"))
        .args([language, workflow.to_str().unwrap(), "--due"])
        .arg(&peer)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--evidence")
        .arg(&evidence)
        .args(["--args", "{}", "--timeout-ms", "10000"])
        .env(
            "RPC_TEST_CASE",
            match case {
                "permission_missing" | "reply_on_item" | "retired_reply" => "permission",
                "sticky_session" => "session_failed",
                other => other,
            },
        )
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.success(),
        success,
        "{language}/{case}: {}\n{stderr}",
        output.status
    );
    assert!(output.stdout.is_empty(), "guest logs must use stderr");
    assert!(
        stderr.contains("FIXTURE_EOF"),
        "{language}/{case}: {stderr}"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["exit_code"], 0, "{language}/{case}: {receipt}");
    assert_eq!(receipt["settled"], true, "{language}/{case}: {receipt}");
    assert_eq!(
        receipt["forced_kill"], false,
        "{language}/{case}: {receipt}"
    );
    let raw = fs::read(evidence.join("stdout.bin")).unwrap();
    assert!(!raw.is_empty());
    if success {
        assert!(
            stderr.contains("WORKFLOW_OK"),
            "{language}/{case}: {stderr}"
        );
        assert!(receipt["transport_error"].is_null(), "{receipt}");
    } else {
        assert!(!stderr.contains("WORKFLOW_OK"));
        assert!(
            !stderr.contains("expected failure was not raised"),
            "the expected failure must actually occur: {language}/{case}: {stderr}"
        );
        let expected: &[&str] = match case {
            "session_failed" => &["session failed", "session_failed"],
            "gap" => &["gap"],
            "omitted" => &["omitt"],
            "bad_json" => &["json", "key must be a string", "property name"],
            "bad_utf8" => &["utf-8", "utf8"],
            "callback" => &["fixture observer failed"],
            "reentrant" => &["reentrant", "blocking"],
            "permission_missing" => &["handler", "permission"],
            "async_callback" => &["synchronous", "async", "thenable"],
            "invalid_id" => &["response id", "response identity", "unknown id"],
            "invalid_version" => &["protocol", "version"],
            "unsafe_outbound" => &["unsafe", "safe integer"],
            "discarded_rejection" => &["rejection", "asynchronous", "async"],
            _ => panic!("missing diagnostic assertion for {case}"),
        };
        let lower = stderr.to_lowercase();
        assert!(
            expected.iter().any(|text| lower.contains(text)),
            "wrong failure for {language}/{case}: {stderr}"
        );
    }
    if case == "shapes" {
        let raw = String::from_utf8(raw).unwrap();
        assert!(raw.contains("9007199254740993"));
        assert!(raw.contains("18446744073709551615"));
    } else if case == "bad_utf8" {
        assert_eq!(raw, b"{\"bad\":\"\xff\"}\n");
    } else if case == "session_failed" || case == "sticky_session" {
        assert!(stderr.contains("ITEM:session_failed"));
    }
}

#[test]
fn both_languages_keep_turns_items_and_json_shapes() {
    for language in languages() {
        for case in [
            "conversation",
            "shapes",
            "mutation",
            "out_of_order",
            "failed_turn",
            "rpc_error",
        ] {
            run_case(language, case, true);
        }
    }
}

#[test]
fn both_languages_deny_defer_interrupt_and_reuse() {
    for language in languages() {
        for case in ["permission", "interrupt", "reply_on_item", "retired_reply"] {
            run_case(language, case, true);
        }
        run_case(language, "permission_missing", false);
    }
}

#[test]
fn both_languages_fail_visibly_and_join_the_child() {
    for language in languages() {
        for case in [
            "session_failed",
            "gap",
            "omitted",
            "bad_json",
            "bad_utf8",
            "callback",
            "reentrant",
            "async_callback",
            "invalid_id",
            "invalid_version",
        ] {
            run_case(language, case, false);
        }
    }
}

#[test]
fn both_languages_preserve_first_failure_after_it_is_caught() {
    for language in languages() {
        for case in ["sticky_session", "sticky_callback"] {
            run_case(language, case, true);
        }
    }
}

#[cfg(feature = "js")]
#[test]
fn javascript_rejects_unsafe_outbound_numbers() {
    run_case("js", "unsafe_outbound", false);
}

#[cfg(feature = "js")]
#[test]
fn javascript_rejects_discarded_promise_failures() {
    run_case("js", "discarded_rejection", false);
}

// Exercise the runtime/bridge lifetime separately from the guest client's state.
// The external timeout and emergency cleanup prevent a broken interpreter hook
// or signal wake-up from leaving an unbounded test or a forgotten peer process.
fn run_lifecycle(language: &str, mode: &str, signal: Option<&str>) {
    let dir = tempfile::tempdir().unwrap();
    let peer = dir.path().join("peer");
    fs::write(&peer, include_bytes!("peer.py")).unwrap();
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o755)).unwrap();
    let ready = dir.path().join("ready");
    let evidence = dir.path().join("evidence");
    let workflow = dir.path().join(format!("workflow.{language}"));
    let prefix = if language == "js" {
        "const c=new DueClient(); c.open({provider:'codex'});\n"
    } else {
        "local c=DueClient.new(); c:open({provider='codex'})\n"
    };
    let body = match (language, mode) {
        ("js", "throw") => "throw new Error('fixture script failed');",
        ("lua", "throw") => "error('fixture script failed')",
        ("js", "loop") => "for(;;) {}",
        ("lua", "loop") => "while true do end",
        ("js", "caught_loop") => "try { for(;;) {} } catch(e) {}",
        ("lua", "caught_loop") => "pcall(function() while true do end end)",
        ("js", "receive") => "for(;;) bridge.recv(10000);",
        ("lua", "receive") => "while true do bridge.recv(10000) end",
        _ => panic!("unsupported lifecycle mode"),
    };
    fs::write(&workflow, format!("{prefix}{body}\n")).unwrap();
    let mut runner = Command::new(env!("CARGO_BIN_EXE_due-rpc-languages"))
        .args([language, workflow.to_str().unwrap(), "--due"])
        .arg(&peer)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--evidence")
        .arg(&evidence)
        .args([
            "--args",
            "{}",
            "--timeout-ms",
            if signal.is_none() && mode != "throw" {
                "1000"
            } else {
                "10000"
            },
        ])
        .env("RPC_TEST_CASE", "conversation")
        .env("RPC_TEST_READY", &ready)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    if let Some(signal) = signal {
        while !ready.is_file() && start.elapsed() < Duration::from_secs(5) {
            if runner.try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if ready.is_file() {
            assert!(
                Command::new("kill")
                    .args([&format!("-{signal}"), &runner.id().to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut timed_out = false;
    loop {
        if runner.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = runner.kill();
            if let Ok(pid) = fs::read_to_string(&ready) {
                let _ = Command::new("kill").args(["-TERM", pid.trim()]).status();
            }
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = runner.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !timed_out,
        "runtime/transport did not stop: {language}/{mode}/{signal:?}: {stderr}"
    );
    assert!(ready.is_file(), "fixture never became ready: {stderr}");
    assert!(!output.status.success(), "{language}/{mode}: {stderr}");
    let expected = match signal {
        Some("TERM") => "runner received SIGTERM",
        Some("INT") => "runner received SIGINT",
        Some("HUP") => "runner received SIGHUP",
        Some(other) => panic!("unsupported signal {other}"),
        None if mode == "throw" => "fixture script failed",
        None => "workflow deadline exceeded",
    };
    assert!(stderr.contains(expected), "{language}/{mode}: {stderr}");
    assert!(
        stderr.contains("FIXTURE_EOF"),
        "{language}/{mode}: {stderr}"
    );
    assert!(output.stdout.is_empty());
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["exit_code"], 0, "{receipt}");
    assert_eq!(receipt["settled"], true, "{receipt}");
    assert_eq!(receipt["forced_kill"], false, "{receipt}");
}

#[test]
fn both_languages_settle_after_script_errors_and_deadlines() {
    for language in languages() {
        for mode in ["throw", "loop", "caught_loop", "receive"] {
            run_lifecycle(language, mode, None);
        }
    }
}

#[test]
fn both_languages_settle_after_signals_in_receive_and_cpu_work() {
    for language in languages() {
        for signal in ["TERM", "INT", "HUP"] {
            for mode in ["receive", "loop"] {
                run_lifecycle(language, mode, Some(signal));
            }
        }
    }
}
