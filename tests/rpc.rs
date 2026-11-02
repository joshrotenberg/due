#![cfg(all(feature = "rpc", unix))]

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Client {
    dir: TempDir,
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    notifications: Vec<Value>,
    responses: BTreeMap<String, Value>,
    next_id: u64,
    close_stdout: Arc<AtomicBool>,
}

impl Client {
    fn new(early: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("provider");
        fs::write(&binary, include_bytes!("fixtures/session-agent.sh")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("rpc")
            .env("DUE_SESSION_FIXTURE_DIR", dir.path())
            .env("DUE_SESSION_EARLY", if early { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        let close_stdout = Arc::new(AtomicBool::new(false));
        let reader_close = close_stdout.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if reader_close.load(Ordering::Acquire) {
                    break;
                }
                let line = line.unwrap();
                let message = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("non-protocol stdout {line:?}: {error}"));
                if tx.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            dir,
            child,
            stdin,
            messages,
            notifications: Vec::new(),
            responses: BTreeMap::new(),
            next_id: 0,
            close_stdout,
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn write(&mut self, value: Value) {
        self.raw(format!("{value}\n").as_bytes());
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = json!(self.next_id);
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        id
    }

    fn response(&mut self, id: &Value, timeout: Duration) -> Value {
        let key = id.to_string();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(response) = self.responses.remove(&key) {
                return response;
            }
            let message = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("RPC {id} did not respond: {error}"));
            assert_eq!(message["jsonrpc"], "2.0", "{message}");
            if let Some(id) = message.get("id") {
                assert_ne!(
                    message.get("result").is_some(),
                    message.get("error").is_some(),
                    "{message}"
                );
                assert!(
                    self.responses.insert(id.to_string(), message).is_none(),
                    "duplicate response ID"
                );
            } else {
                assert_eq!(message["method"], "session.item", "{message}");
                self.notifications.push(message);
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        self.response(&id, Duration::from_secs(15))
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params);
        assert!(response.get("error").is_none(), "{method}: {response}");
        response["result"].clone()
    }

    fn open(&mut self, provider: &str, stream: bool, timeout: Option<&str>) -> String {
        let mut options = json!({"provider":provider,"binary":self.dir.path().join("provider"),"cwd":self.dir.path(),"stream":stream});
        if let Some(timeout) = timeout {
            options["timeout"] = json!(timeout);
        }
        let response = self.ok("session.open", options);
        assert_eq!(response["protocol_version"], 1);
        assert_eq!(response["stream"]["enabled"], stream);
        assert_eq!(response["stream"]["method"], "session.item");
        response["session_id"].as_str().unwrap().into()
    }

    fn pushed_until(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(item) = self.notifications.iter().find_map(|message| {
                predicate(&message["params"]).then(|| message["params"].clone())
            }) {
                return item;
            }
            let message = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!("session item not pushed: {error}; {:?}", self.notifications)
                });
            if let Some(id) = message.get("id") {
                assert!(self.responses.insert(id.to_string(), message).is_none());
            } else {
                assert_eq!(message["method"], "session.item", "{message}");
                self.notifications.push(message);
            }
        }
    }

    fn finished(&mut self, session: &str, turn: &str) -> Value {
        self.pushed_until(|item| {
            item["session_id"] == session
                && item["turn_id"] == turn
                && item["kind"] == "turn_finished"
        })["data"]
            .clone()
    }

    fn start(&mut self, session: &str, prompt: &str) -> String {
        self.ok("turn.start", json!({"session_id":session,"prompt":prompt}))["turn_id"]
            .as_str()
            .unwrap()
            .into()
    }

    fn read_until(&mut self, session: &str, turn: &str, state: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut after = 0;
        loop {
            let value = self.ok(
                "turn.read",
                json!({"session_id":session,"turn_id":turn,"after":after,"wait_ms":100}),
            );
            if value["state"] == state {
                return value;
            }
            assert_ne!(value["state"], "finished", "unexpected completion: {value}");
            assert!(
                Instant::now() < deadline,
                "turn never reached {state}: {value}"
            );
            after = value["cursor"].as_u64().unwrap();
        }
    }

    fn ready(&mut self, provider: &str, session: &str, turn: &str) {
        self.pushed_until(|item| {
            item["session_id"] == session
                && item["turn_id"] == turn
                && if provider == "codex" {
                    item["kind"] == "native_response" && item["data"]["method"] == "turn/start"
                } else {
                    item["kind"] == "native" && item["data"]["event"]["type"] == "assistant"
                }
        });
    }

    fn close(&mut self, session: &str) {
        assert_eq!(
            self.ok("session.close", json!({"session_id":session}))["closed"],
            true
        );
        assert!(
            self.dir.path().join("exited").exists(),
            "native process did not observe shutdown"
        );
    }

    fn eof(&mut self) {
        self.stdin.take();
        self.wait_exit(true);
    }

    fn wait_exit(&mut self, success: bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(status.success(), success, "unexpected RPC exit: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "RPC process did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stdin.take();
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if !self.dir.path().join("exited").exists()
            && let Ok(pid) = fs::read_to_string(self.dir.path().join("pid"))
            && let Ok(pid) = pid.trim().parse::<i32>()
        {
            // Only a fixture created by this test can write this private PID file.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

#[test]
fn no_handshake_two_turns_one_native_session_and_push_is_opt_in() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, false, None);
        assert_eq!(
            client.call("session.open", json!({"provider":provider}))["error"]["code"],
            -32000
        );
        for number in 1..=2 {
            let turn = client.start(&session, "ordinary");
            let value = client.read_until(&session, &turn, "finished");
            assert_eq!(value["result"]["status"], "succeeded", "{value}");
            assert_eq!(value["result"]["text"], format!("turn {number}"));
            assert_eq!(value["result"]["session_id"], format!("native-{provider}"));
            assert_eq!(value["result"]["run_id"], turn);
        }
        client.close(&session);
        client.eof();
        client.notifications.extend(client.messages.try_iter());
        assert!(client.notifications.is_empty(), "push must be opt-in");
    }
}

#[test]
fn full_native_items_stream_during_a_held_turn_and_replay_losslessly() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        let native = client.pushed_until(|item| {
            item["session_id"] == session
                && item["kind"] == "native"
                && if provider == "claude" {
                    item["data"]["event"]["type"] == "assistant"
                } else {
                    item["data"]["method"] == "item/started"
                }
        });
        let payload = if provider == "claude" {
            &native["data"]["event"]["fixturePayload"]
        } else {
            &native["data"]["params"]["item"]["fixturePayload"]
        };
        assert_eq!(
            payload,
            &json!({"nested":[{"input":{"path":"a/b","flags":[true,null,3]},"output":{"stdout":"line one\nline two","opaque":{"future":42}}}]})
        );
        assert_eq!(native["push_dropped"], 0);
        assert!(
            !client
                .notifications
                .iter()
                .any(|m| m["params"]["kind"] == "turn_finished")
        );
        let replay = client.ok("session.read", json!({"session_id":session}));
        assert_eq!(replay["items_lost"], false);
        let same = replay["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["cursor"] == native["cursor"])
            .unwrap();
        assert_eq!(same["data"], native["data"]);
        client.ready(provider, &session, &turn);
        assert_eq!(
            client.call(
                "turn.start",
                json!({"session_id":session,"prompt":"ordinary"})
            )["error"]["code"],
            -32000
        );
        client.ok(
            "turn.interrupt",
            json!({"session_id":session,"turn_id":turn}),
        );
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        let next = client.start(&session, "ordinary");
        let completed = client.finished(&session, &next);
        assert_eq!(completed["text"], "turn 2");
        assert_eq!(completed["session_id"], format!("native-{provider}"));
        client.close(&session);
        client.eof();
    }
}

#[test]
fn pending_long_poll_does_not_block_interrupt_or_its_response() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        let snapshot = client.ok("turn.read", json!({"session_id":session,"turn_id":turn}));
        let read_id = client.request(
            "turn.read",
            json!({"session_id":session,"turn_id":turn,"after":snapshot["cursor"],"wait_ms":10000}),
        );
        let interrupt_id = client.request(
            "turn.interrupt",
            json!({"session_id":session,"turn_id":turn}),
        );
        assert_eq!(
            client.response(&interrupt_id, Duration::from_secs(3))["result"]["interrupt_requested"],
            true
        );
        let read = client.response(&read_id, Duration::from_secs(3));
        assert!(read.get("error").is_none(), "{read}");
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        client.close(&session);
        client.eof();
    }
}

#[test]
fn streamed_permission_request_requires_explicit_reply_and_rejects_stale_reply() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "ask");
        let interaction = client.pushed_until(|item| {
            item["session_id"] == session
                && item["turn_id"] == turn
                && item["kind"] == "interaction"
        });
        let request = interaction["data"]["request_id"].clone();
        let response = if provider == "claude" {
            json!({"behavior":"deny","message":"Fixture denial"})
        } else {
            json!({"decision":"cancel"})
        };
        let args =
            json!({"session_id":session,"turn_id":turn,"request_id":request,"response":response});
        assert_eq!(
            client.ok("interaction.reply", args.clone())["replied"],
            true
        );
        assert_eq!(
            client.finished(&session, &turn)["text"],
            "permission denied"
        );
        assert_eq!(
            client.call("interaction.reply", args)["error"]["code"],
            -32000
        );
        assert!(!client.dir.path().join("never-created").exists());
        let replay = client.ok("session.read", json!({"session_id":session}));
        let records = replay["items"].as_array().unwrap();
        let position = |kind: &str| {
            records
                .iter()
                .position(|item| item["kind"] == kind)
                .unwrap()
        };
        assert!(position("interaction") < position("interaction_reply"));
        assert!(position("interaction_reply") < position("interaction_resolved"));
        assert!(position("interaction_resolved") < position("turn_finished"));
        client.close(&session);
        client.eof();
    }
}

#[test]
fn failed_native_turn_is_result_data_and_session_accepts_follow_up() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "fail");
        let result = client.finished(&session, &turn);
        assert_eq!(result["status"], "failed", "{result}");
        assert_eq!(result["error"]["kind"], "provider");
        let next = client.start(&session, "ordinary");
        assert_eq!(client.finished(&session, &next)["status"], "succeeded");
        client.close(&session);
        client.eof();
    }
}

#[test]
fn eof_cleans_up_active_native_session_and_drains_terminal_items() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        client.eof();
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        client
            .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_closed");
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived EOF"
        );
    }
}

#[test]
fn oversized_frame_stops_native_process() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        let mut oversized = vec![b'x'; 1024 * 1024 + 1];
        oversized.push(b'\n');
        // Rejection may occur before the writer reaches the end of the frame.
        let _ = client.stdin.as_mut().unwrap().write_all(&oversized);
        let _ = client.stdin.as_mut().unwrap().flush();
        client.wait_exit(false);
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived invalid frame"
        );
    }
}

#[test]
fn native_deadline_reports_timeout_after_cleanup() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, Some("200ms"));
        let turn = client.start(&session, "hold");
        let result = client.finished(&session, &turn);
        assert_eq!(result["status"], "timed_out", "{result}");
        assert_eq!(result["error"]["kind"], "timeout");
        assert!(
            client.dir.path().join("exited").exists(),
            "timeout reported before cleanup"
        );
        assert_eq!(
            client.call(
                "turn.start",
                json!({"session_id":session,"prompt":"ordinary"})
            )["error"]["code"],
            -32000
        );
        client.call("session.close", json!({"session_id":session}));
        client.eof();
    }
}

#[test]
fn early_completion_and_pagination_keep_both_turns_and_unknown_native_fields() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(true);
        let session = client.open(provider, true, None);
        let mut turns = Vec::new();
        for number in 1..=2 {
            let turn = client.start(&session, "ordinary");
            assert_eq!(
                client.finished(&session, &turn)["text"],
                format!("turn {number}")
            );
            turns.push(turn);
        }
        let mut after = 0;
        let mut records = Vec::new();
        loop {
            let page = client.ok(
                "session.read",
                json!({"session_id":session,"after":after,"limit":2}),
            );
            assert_eq!(page["items_lost"], false);
            assert_eq!(page["dropped_items"], 0);
            let items = page["items"].as_array().unwrap();
            assert!(items.len() <= 2);
            for item in items {
                assert_eq!(item["cursor"], after + 1);
                assert_eq!(item["session_id"], session);
                after += 1;
                records.push(item.clone());
            }
            assert_eq!(page["cursor"], after);
            if page["has_more"] == false {
                break;
            }
            assert!(!items.is_empty());
        }
        for turn in turns {
            for kind in ["turn_started", "turn_finished"] {
                assert!(
                    records
                        .iter()
                        .any(|item| item["kind"] == kind && item["turn_id"] == turn)
                );
            }
        }
        if provider == "codex" {
            assert!(records.iter().any(|item| item["kind"] == "native_response"
                && item["data"]["method"] == "thread/start"
                && item["data"]["response"]["thread"]["fixtureThread"]["unknown"][1]["keep"]
                    == true));
            assert!(records.iter().any(|item| item["kind"] == "native"
                && item["data"]["method"] == "fixture/idle"
                && item["turn_id"].is_null()));
        } else {
            assert!(records.iter().any(|item| item["kind"] == "native_snapshot"
                && item["data"]["event"]["fixtureInit"]["unknown"] == true));
        }
        assert_eq!(
            client.call(
                "session.read",
                json!({"session_id":session,"after":after + 1})
            )["error"]["code"],
            -32000
        );
        client.close(&session);
        client.eof();
    }
}

#[test]
fn protocol_errors_are_structured_and_connection_remains_usable() {
    let mut client = Client::new(false);
    client.raw(b"{broken\n");
    assert_eq!(
        client.response(&Value::Null, Duration::from_secs(3))["error"]["code"],
        -32700
    );
    for invalid in [
        json!([]),
        json!([{"jsonrpc":"2.0","id":1,"method":"missing"}]),
        json!(42),
        json!({"jsonrpc":"1.0","id":{},"method":"missing"}),
    ] {
        client.write(invalid);
        assert_eq!(
            client.response(&Value::Null, Duration::from_secs(3))["error"]["code"],
            -32600
        );
    }
    assert_eq!(client.call("missing", json!({}))["error"]["code"], -32601);
    assert_eq!(
        client.call("session.open", json!({"provider":"invalid"}))["error"]["code"],
        -32602
    );
    assert_eq!(
        client.call("turn.start", json!({"session_id":"missing"}))["error"]["code"],
        -32602
    );
    assert_eq!(
        client.call("session.read", json!({"session_id":"missing","extra":true}))["error"]["code"],
        -32602
    );
    let session = client.open("claude", false, None);
    client.close(&session);
    client.eof();
}

#[test]
fn ids_round_trip_and_notifications_never_receive_responses() {
    let mut client = Client::new(false);
    client.write(json!({"jsonrpc":"2.0","method":"missing","params":{}}));
    client.write(json!({"jsonrpc":"2.0","method":"session.open","params":{"provider":"invalid"}}));
    for id in [json!("string-id"), json!(-42), Value::Null] {
        client.write(json!({"jsonrpc":"2.0","id":id,"method":"missing"}));
        let response = client.response(&id, Duration::from_secs(3));
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], -32601);
    }
    let session = client.open("claude", true, None);
    client.write(json!({"jsonrpc":"2.0","method":"turn.start","params":{"session_id":session,"prompt":"ordinary"}}));
    let started = client.pushed_until(|item| item["kind"] == "turn_started");
    let turn = started["turn_id"].as_str().unwrap();
    assert_eq!(client.finished(&session, turn)["status"], "succeeded");
    client.close(&session);
    client.eof();
    // EOF closes the reader channel after every output line was delivered.
    for message in client.messages.iter() {
        assert!(
            message.get("id").is_none(),
            "notification received response: {message}"
        );
    }
    assert!(
        client.responses.is_empty(),
        "unrequested responses: {:?}",
        client.responses
    );
}

#[test]
fn sigterm_exits_with_stdin_still_open_and_settles_native_process() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        // Signal only the Due process owned by this test. Keep its input pipe
        // open to ensure shutdown does not rely on a cooperative caller's EOF.
        let sent = unsafe { libc::kill(client.child.id() as i32, libc::SIGTERM) };
        assert_eq!(
            sent,
            0,
            "SIGTERM failed: {}",
            std::io::Error::last_os_error()
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = client.child.try_wait().unwrap() {
                assert!(status.success(), "signal shutdown failed: {status}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Due survived SIGTERM with stdin held open; native exit observed: {}",
                client.dir.path().join("exited").exists()
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(client.stdin.is_some());
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived SIGTERM"
        );
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        client
            .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_closed");
    }
}

#[test]
fn broken_stdout_stops_due_and_native_process_with_stdin_still_open() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        // Wake the reader with one harmless reply, then let it drop the owned
        // stdout pipe. No cross-thread raw descriptor closing is needed.
        client.close_stdout.store(true, Ordering::Release);
        client.request("missing", json!({}));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match client
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => panic!("stdout reader did not close: {error}"),
            }
        }
        client.request("missing", json!({}));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = client.child.try_wait().unwrap() {
                assert!(!status.success(), "broken stdout must fail the connection");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Due survived broken stdout with stdin held open; native exit observed: {}",
                client.dir.path().join("exited").exists()
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(client.stdin.is_some());
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived broken stdout"
        );
    }
}

#[test]
fn stalled_stdout_times_out_with_stdin_and_stdout_still_open() {
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if self.0.try_wait().unwrap().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    // Keep the output reader alive without consuming any bytes. The echoed ID
    // exceeds the pipe's buffer, so this tests a stalled write, not EPIPE.
    let _output = child.0.stdout.take().unwrap();
    let mut stderr = child.0.stderr.take().unwrap();
    let (written, received) = mpsc::channel();
    thread::spawn(move || {
        let request = json!({"jsonrpc":"2.0","id":"x".repeat(256 * 1024),"method":"missing"});
        let result = writeln!(input, "{request}").and_then(|()| input.flush());
        let _ = written.send((input, result));
    });
    // Bound even fixture input submission. OwnedChild kills/reaps Due if any
    // assertion fails, which also releases a blocked input-writing thread.
    let (_input, result) = received
        .recv_timeout(Duration::from_secs(3))
        .expect("Due did not consume the request");
    result.unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(!status.success(), "stalled stdout must fail the connection");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Due survived stalled stdout with both pipes held open"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let mut error = String::new();
    std::io::Read::read_to_string(&mut stderr, &mut error).unwrap();
    assert!(
        error.contains("stdout stalled") || error.contains("output drain timed out"),
        "expected a bounded output timeout, got: {error}"
    );
}

#[test]
fn codex_early_requests_wait_for_authoritative_turn_and_replay_in_order() {
    let mut client = Client::new(true);
    let session = client.open("codex", true, None);
    let first = client.start(&session, "normal");
    assert_eq!(client.finished(&session, &first)["status"], "succeeded");
    let second = client.start(&session, "stale-approval");
    let raw =
        client.pushed_until(|item| item["kind"] == "native_request" && item["data"]["id"] == 18);
    assert!(raw["turn_id"].is_null(), "{raw}");
    assert_eq!(
        raw["data"]["params"]["unknown"],
        json!({"keep":[true,null,42]})
    );
    let read = client.ok("turn.read", json!({"session_id":session,"turn_id":second}));
    assert!(
        read["interactions"].as_array().unwrap().is_empty(),
        "{read}"
    );
    client.pushed_until(|item| {
        item["turn_id"] == second
            && item["kind"] == "native_response"
            && item["data"]["method"] == "turn/start"
    });
    // Wait for a correlated notification to prove the ordered replay has run.
    client.read_until(&session, &second, "running");
    let read = client.ok("turn.read", json!({"session_id":session,"turn_id":second}));
    assert!(
        read["interactions"].as_array().unwrap().is_empty(),
        "{read}"
    );
    client.ok(
        "turn.interrupt",
        json!({"session_id":session,"turn_id":second}),
    );
    client.finished(&session, &second);
    let third = client.start(&session, "ask");
    let interaction =
        client.pushed_until(|item| item["turn_id"] == third && item["kind"] == "interaction");
    assert_eq!(interaction["data"]["request_id"], "codex:17");
    client.ok("interaction.reply", json!({"session_id":session,"turn_id":third,"request_id":"codex:17","response":{"decision":"cancel"}}));
    assert_eq!(
        client.finished(&session, &third)["text"],
        "permission denied"
    );
    let fourth = client.start(&session, "ask-resolved");
    assert_eq!(
        client.finished(&session, &fourth)["text"],
        "resolved before start"
    );
    let read = client.ok("turn.read", json!({"session_id":session,"turn_id":fourth}));
    assert!(
        read["interactions"].as_array().unwrap().is_empty(),
        "{read}"
    );
    let fifth = client.start(&session, "ask-completed");
    assert_eq!(
        client.finished(&session, &fifth)["text"],
        "completed with pending request"
    );
    let read = client.ok("turn.read", json!({"session_id":session,"turn_id":fifth}));
    assert!(
        read["interactions"].as_array().unwrap().is_empty(),
        "{read}"
    );
    let records = client.ok("session.read", json!({"session_id":session}));
    let items = records["items"].as_array().unwrap();
    assert!(
        !items
            .iter()
            .any(|i| i["turn_id"] == second && i["kind"] == "interaction")
    );
    let position = |kind| {
        items
            .iter()
            .position(|i| i["turn_id"] == fourth && i["kind"] == kind)
            .unwrap()
    };
    assert!(position("interaction") < position("interaction_resolved"));
    assert!(position("interaction_resolved") < position("turn_finished"));
    client.close(&session);
    client.eof();
    let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
    let rejection = input
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["id"] == 18 && v.get("error").is_some())
        .unwrap();
    assert_eq!(
        rejection["error"]["message"],
        "Request belongs to another turn"
    );
    for (id, message) in [(17, "Due turn has finished"), (19, "No active Due turn")] {
        assert!(
            input
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .any(|v| v["id"] == id && v["error"]["message"] == message)
        );
    }
}

#[test]
fn codex_limits_unanswered_early_requests_before_start_response() {
    for prompt in [
        "early-overflow",
        "early-foreign-resolution",
        "early-release",
    ] {
        let mut client = Client::new(true);
        let session = client.open("codex", true, None);
        let turn = client.start(&session, prompt);
        let raw = client.pushed_until(|i| i["kind"] == "native_request" && i["data"]["id"] == 133);
        assert!(raw["turn_id"].is_null());
        if prompt == "early-release" {
            fs::write(client.dir.path().join("release-start"), "release").unwrap();
            client.pushed_until(|i| {
                i["kind"] == "interaction" && i["data"]["request_id"] == "codex:133"
            });
            let read = client.ok("turn.read", json!({"session_id":session,"turn_id":turn}));
            let pending = read["interactions"].as_array().unwrap();
            assert_eq!(pending.len(), 32, "{read}");
            assert!(!pending.iter().any(|i| i["request_id"] == "codex:101"));
            assert!(pending.iter().any(|i| i["request_id"] == "codex:133"));
            client.ok(
                "turn.interrupt",
                json!({"session_id":session,"turn_id":turn}),
            );
            assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        } else {
            let result = client.finished(&session, &turn);
            assert_eq!(result["status"], "failed", "{result}");
            assert!(
                result["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("32 pending interaction limit"),
                "{result}"
            );
            assert!(client.dir.path().join("exited").exists());
        }
        let records = client.ok("session.read", json!({"session_id":session,"limit":256}));
        let items = records["items"].as_array().unwrap();
        let requests: Vec<_> = items
            .iter()
            .filter(|i| i["kind"] == "native_request")
            .collect();
        assert_eq!(requests.len(), 33);
        for (index, request) in requests.iter().enumerate() {
            assert_eq!(request["data"]["id"], 101 + index);
            assert!(request["turn_id"].is_null());
            assert_eq!(request["data"]["params"]["turnId"], "turn-1");
        }
        if prompt == "early-release" {
            let position = |kind, id: &str| {
                items
                    .iter()
                    .position(|i| i["kind"] == kind && i["data"]["request_id"] == id)
                    .unwrap()
            };
            assert!(
                position("interaction", "codex:101")
                    < position("interaction_resolved", "codex:101")
            );
            assert!(
                position("interaction_resolved", "codex:101")
                    < position("interaction", "codex:133")
            );
        } else {
            assert!(!items.iter().any(|i| i["kind"] == "interaction"));
            assert!(
                !items
                    .iter()
                    .any(|i| i["kind"] == "native_response" && i["data"]["method"] == "turn/start")
            );
        }
        client.close(&session);
        client.eof();
        let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
        let replies: Vec<Value> = input
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v.get("method").is_none())
            .collect();
        assert!(
            !replies.iter().any(|v| v.get("result").is_some()),
            "must never auto-approve: {replies:?}"
        );
        if prompt != "early-release" {
            assert!(
                replies.iter().any(|v| v["id"] == 133
                    && v["error"]["message"] == "Too many pending Due interactions"),
                "{replies:?}"
            );
        }
    }
}

#[test]
fn codex_contradictory_completion_ids_cannot_succeed() {
    for early in [false, true] {
        for prompt in [
            "conflicting-completion",
            "conflicting-reverse",
            "conflicting-answer",
            "conflicting-usage",
        ] {
            let mut client = Client::new(early);
            let session = client.open("codex", true, None);
            let turn = client.start(&session, prompt);
            let result = client.finished(&session, &turn);
            assert_eq!(result["status"], "failed", "{prompt}: {result}");
            assert_eq!(result["error"]["kind"], "protocol", "{result}");
            assert!(
                result["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("contradictory turn IDs")
            );
            assert_eq!(result["text"], "accepted answer");
            assert_eq!(result["usage"]["input_tokens"], 10);
            assert_eq!(result["usage"]["output_tokens"], 2);
            let records = client.ok("session.read", json!({"session_id":session}));
            let raw = records["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| {
                    item["kind"] == "native"
                        && item["data"]["params"]["turnId"].is_string()
                        && item["data"]["params"]["turn"]["id"].is_string()
                        && item["data"]["params"]["turnId"] != item["data"]["params"]["turn"]["id"]
                })
                .expect("contradictory native evidence retained");
            assert!(raw["turn_id"].is_null(), "{raw}");
            client.close(&session);
            client.eof();
        }
    }
}

#[test]
fn codex_consistent_completion_ids_preserve_normal_correlation() {
    for early in [false, true] {
        for prompt in [
            "matching-completion",
            "nested-completion",
            "stale-completion",
        ] {
            let mut client = Client::new(early);
            let session = client.open("codex", true, None);
            let turn = client.start(&session, prompt);
            let result = client.finished(&session, &turn);
            assert_eq!(result["status"], "succeeded", "{prompt}: {result}");
            assert_eq!(
                result["text"],
                if prompt == "stale-completion" {
                    "after stale"
                } else {
                    "accepted answer"
                }
            );
            client.close(&session);
            client.eof();
        }
    }
}

#[test]
fn codex_malformed_identity_values_preserve_accepted_evidence() {
    for early in [false, true] {
        for slot in ["top", "nested"] {
            for value in ["number", "array", "object", "boolean", "null"] {
                for event in ["answer", "usage", "completion"] {
                    // Turn.id is required by the pinned typed completion contract.
                    if value == "null" && (slot != "nested" || event != "completion") {
                        continue;
                    }
                    let prompt = format!("identity-{slot}-{value}-{event}");
                    let mut client = Client::new(early);
                    let session = client.open("codex", true, None);
                    let turn = client.start(&session, &prompt);
                    let result = client.finished(&session, &turn);
                    assert_eq!(result["status"], "failed", "{prompt}: {result}");
                    assert_eq!(result["error"]["kind"], "protocol");
                    assert_eq!(result["text"], "accepted answer", "{prompt}: {result}");
                    assert_eq!(result["usage"]["input_tokens"], 10);
                    let records = client.ok("session.read", json!({"session_id":session}));
                    let raw = records["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|i| {
                            i["kind"] == "native" && i["data"]["params"]["fixtureIdentity"] == true
                        })
                        .expect("raw malformed evidence");
                    if value != "null" {
                        assert!(raw["turn_id"].is_null(), "{raw}");
                    }
                    client.close(&session);
                    client.eof();
                }
            }
        }
    }
}

#[test]
fn codex_nullable_optional_identity_and_unrelated_payloads_remain_usable() {
    for early in [false, true] {
        for prompt in [
            "identity-top-null-completion",
            "identity-nested-null-usage",
            "identity-top-number-foreign-answer",
            "identity-nested-object-foreign-usage",
            "identity-top-number-unknown-answer",
        ] {
            let mut client = Client::new(early);
            let session = client.open("codex", true, None);
            let turn = client.start(&session, prompt);
            let result = client.finished(&session, &turn);
            assert_eq!(result["status"], "succeeded", "{prompt}: {result}");
            client.close(&session);
            client.eof();
        }
    }
}
