#![cfg(all(feature = "mcp", unix))]

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
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
    next_id: u64,
}

impl Client {
    fn new(early: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("provider");
        fs::write(&binary, include_bytes!("fixtures/mcp-agent.sh")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("mcp")
            .env("DUE_MCP_FIXTURE_DIR", dir.path())
            .env("DUE_MCP_EARLY", if early { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let message = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("non-protocol stdout {line:?}: {error}"));
                if tx.send(message).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            dir,
            child,
            stdin,
            messages,
            notifications: Vec::new(),
            next_id: 0,
        };
        let initialized = client.rpc(
            "initialize",
            json!({
                "protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"due-mcp-test","version":"0"},
            }),
        );
        assert_eq!(initialized["protocolVersion"], "2025-11-25");
        client.write(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn write(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &value).unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let message = self
                .messages
                .recv_timeout(Duration::from_secs(15))
                .unwrap_or_else(|e| panic!("MCP {method} did not respond: {e}"));
            if message.get("id") == Some(&json!(id)) {
                assert!(message.get("error").is_none(), "{message}");
                return message["result"].clone();
            }
            assert!(
                message.get("id").is_none(),
                "unexpected response: {message}"
            );
            self.notifications.push(message);
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({"name":name,"arguments":arguments}))
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.call(name, arguments);
        assert_ne!(result["isError"], true, "{name}: {result}");
        result["structuredContent"].clone()
    }

    fn open(&mut self, provider: &str, timeout: Option<&str>) -> String {
        let mut options = json!({"provider":provider,"binary":self.dir.path().join("provider"),"cwd":self.dir.path()});
        if let Some(timeout) = timeout {
            options["timeout"] = json!(timeout);
        }
        self.ok("session_open", options)["session_id"]
            .as_str()
            .unwrap()
            .into()
    }

    fn open_stream(&mut self, provider: &str) -> String {
        self.ok(
            "session_open",
            json!({"provider":provider,"binary":self.dir.path().join("provider"),"cwd":self.dir.path(),"stream":true}),
        )["session_id"]
            .as_str()
            .unwrap()
            .into()
    }

    fn pushed_until(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(item) = self.notifications.iter().find_map(|message| {
                (message["method"] == "notifications/due/session_item"
                    && predicate(&message["params"]))
                .then(|| message["params"].clone())
            }) {
                return item;
            }
            let message = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!("session item not pushed: {error}; {:?}", self.notifications)
                });
            assert!(
                message.get("id").is_none(),
                "unexpected response: {message}"
            );
            self.notifications.push(message);
        }
    }

    fn pushed_finish(&mut self, session: &str, turn: &str) -> Value {
        self.pushed_until(|item| {
            item["session_id"] == session
                && item["turn_id"] == turn
                && item["kind"] == "turn_finished"
        })
    }

    fn start(&mut self, session: &str, prompt: &str) -> String {
        self.ok("turn_start", json!({"session_id":session,"prompt":prompt}))["turn_id"]
            .as_str()
            .unwrap()
            .into()
    }

    fn read_until(&mut self, session: &str, turn: &str, state: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut after = 0;
        loop {
            let value = self.ok(
                "turn_read",
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

    fn close(&mut self, session: &str) {
        assert_eq!(
            self.ok("session_close", json!({"session_id":session}))["closed"],
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
                assert_eq!(status.success(), success, "unexpected MCP exit: {status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "MCP process did not exit after EOF"
            );
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
fn lists_seven_tools_and_keeps_two_turns_in_one_native_session() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let tools = client.rpc("tools/list", json!({}));
        assert_eq!(tools["tools"].as_array().unwrap().len(), 7);
        let session = client.open(provider, None);
        let duplicate = client.call("session_open", json!({"provider":provider}));
        assert_eq!(duplicate["isError"], true);
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
        assert!(
            client
                .notifications
                .iter()
                .all(|message| message["method"] != "notifications/due/session_item"),
            "push must be opt-in"
        );
    }
}

#[test]
fn interrupts_a_held_turn_and_accepts_a_follow_up() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, None);
        let turn = client.start(&session, "hold");
        let duplicate = client.call(
            "turn_start",
            json!({"session_id":session,"prompt":"ordinary"}),
        );
        assert_eq!(duplicate["isError"], true);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let result = client.call(
                "turn_interrupt",
                json!({"session_id":session,"turn_id":turn}),
            );
            if result["isError"] != true {
                break;
            }
            // Codex may not yet have answered turn/start with its native ID.
            assert!(
                result.to_string().contains("No native turn ID yet"),
                "{result}"
            );
            assert!(
                Instant::now() < deadline,
                "no native turn ID became available"
            );
            client.ok(
                "turn_read",
                json!({"session_id":session,"turn_id":turn,"wait_ms":100}),
            );
        }
        let result = client.read_until(&session, &turn, "finished");
        assert_eq!(result["result"]["status"], "cancelled", "{result}");
        let next = client.start(&session, "ordinary");
        let result = client.read_until(&session, &next, "finished");
        assert_eq!(result["result"]["text"], "turn 2");
        assert_eq!(result["result"]["status"], "succeeded");
        client.close(&session);
        client.eof();
    }
}

#[test]
fn permission_denial_is_explicit_and_stale_replies_are_rejected() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, None);
        let turn = client.start(&session, "ask");
        let waiting = client.read_until(&session, &turn, "awaiting_input");
        assert_eq!(waiting["interactions"].as_array().unwrap().len(), 1);
        let request = waiting["interactions"][0]["request_id"].clone();
        let response = if provider == "claude" {
            json!({"behavior":"deny","message":"Fixture denial"})
        } else {
            json!({"decision":"cancel"})
        };
        let args =
            json!({"session_id":session,"turn_id":turn,"request_id":request,"response":response});
        assert_eq!(
            client.ok("interaction_reply", args.clone())["replied"],
            true
        );
        let finished = client.read_until(&session, &turn, "finished");
        assert_eq!(finished["result"]["text"], "permission denied");
        assert!(finished["interactions"].as_array().unwrap().is_empty());
        assert_eq!(client.call("interaction_reply", args)["isError"], true);
        assert!(!client.dir.path().join("never-created").exists());
        client.close(&session);
        client.eof();
    }
}

#[test]
fn native_failed_turns_keep_failed_status() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, None);
        let turn = client.start(&session, "fail");
        let result = client.read_until(&session, &turn, "finished");
        assert_eq!(result["result"]["status"], "failed", "{result}");
        assert_eq!(result["result"]["error"]["kind"], "provider");
        client.close(&session);
        client.eof();
    }
}

#[test]
fn stdin_eof_settles_an_active_native_session() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open_stream(provider);
        let turn = client.start(&session, "hold");
        client.ok(
            "turn_read",
            json!({"session_id":session,"turn_id":turn,"wait_ms":100}),
        );
        client.eof();
        assert_eq!(
            client.pushed_finish(&session, &turn)["data"]["status"],
            "cancelled"
        );
        client
            .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_closed");
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived EOF"
        );
    }
}

#[test]
fn codex_completion_before_start_response_is_retained() {
    let mut client = Client::new(true);
    let session = client.open("codex", None);
    for number in 1..=2 {
        let turn = client.start(&session, "ordinary");
        let result = client.read_until(&session, &turn, "finished");
        assert_eq!(result["result"]["status"], "succeeded", "{result}");
        assert_eq!(result["result"]["text"], format!("turn {number}"));
    }
    client.close(&session);
    client.eof();
}

#[test]
fn oversized_mcp_input_stops_the_native_process() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, None);
        let turn = client.start(&session, "hold");
        client.ok(
            "turn_read",
            json!({"session_id":session,"turn_id":turn,"wait_ms":100}),
        );
        let mut oversized = vec![b'x'; 1024 * 1024 + 1];
        oversized.push(b'\n');
        // The server may reject the frame before the writer reaches its end.
        let _ = client.stdin.as_mut().unwrap().write_all(&oversized);
        let _ = client.stdin.as_mut().unwrap().flush();
        client.wait_exit(false);
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived invalid transport input"
        );
    }
}

#[test]
fn turn_deadline_closes_the_session_before_reporting_timeout() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, Some("200ms"));
        let turn = client.start(&session, "hold");
        let result = client.read_until(&session, &turn, "finished");
        assert_eq!(result["result"]["status"], "timed_out", "{result}");
        assert_eq!(result["result"]["error"]["kind"], "timeout");
        assert!(
            client.dir.path().join("exited").exists(),
            "timeout was reported before native cleanup"
        );
        assert_eq!(
            client.call(
                "turn_start",
                json!({"session_id":session,"prompt":"ordinary"})
            )["isError"],
            true
        );
        // Release the failed session handle. Its close may repeat the recorded
        // provider error, but the MCP connection remains usable for shutdown.
        client.call("session_close", json!({"session_id":session}));
        client.eof();
    }
}

#[test]
fn pushes_full_native_items_while_a_turn_is_still_running() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open_stream(provider);
        let turn = client.start(&session, "hold");
        // No turn_read call: a held native turn must stream before completion.
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
                .any(|message| message["params"]["kind"] == "turn_finished")
        );
        let recovered = client.ok("session_read", json!({"session_id":session}));
        let same = recovered["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["cursor"] == native["cursor"])
            .unwrap();
        assert_eq!(same["data"], native["data"]);
        assert_eq!(recovered["items_lost"], false);
        if provider == "codex" {
            client.pushed_until(|item| {
                item["session_id"] == session
                    && item["turn_id"] == turn
                    && item["kind"] == "native_response"
                    && item["data"]["method"] == "turn/start"
            });
        }
        client.ok(
            "turn_interrupt",
            json!({"session_id":session,"turn_id":turn}),
        );
        let finished = client.pushed_finish(&session, &turn);
        assert_eq!(finished["data"]["status"], "cancelled");
        client.close(&session);
        client.eof();
    }
}

#[test]
fn session_read_replays_both_turns_and_pages_without_skipping_items() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open_stream(provider);
        let first = client.start(&session, "ordinary");
        let first_result = client.pushed_finish(&session, &first);
        assert_eq!(first_result["data"]["text"], "turn 1");
        let second = client.start(&session, "ordinary");
        let second_result = client.pushed_finish(&session, &second);
        assert_eq!(second_result["data"]["text"], "turn 2");
        assert!(
            second_result["cursor"].as_u64().unwrap() > first_result["cursor"].as_u64().unwrap()
        );

        let mut after = 0;
        let mut items = Vec::new();
        loop {
            let page = client.ok(
                "session_read",
                json!({"session_id":session,"after":after,"limit":2}),
            );
            assert_eq!(page["items_lost"], false);
            assert_eq!(page["dropped_items"], 0);
            let records = page["items"].as_array().unwrap();
            assert!(records.len() <= 2);
            for item in records {
                assert_eq!(item["session_id"], session);
                assert_eq!(item["cursor"].as_u64().unwrap(), after + 1);
                after += 1;
                items.push(item.clone());
            }
            assert_eq!(page["cursor"], after);
            if page["has_more"] == false {
                assert_eq!(page["latest_cursor"], after);
                break;
            }
            assert!(!records.is_empty(), "pagination made no progress");
        }
        for turn in [&first, &second] {
            assert!(items.iter().any(|item| item["kind"] == "turn_started"
                && item["turn_id"] == *turn
                && item["data"]["prompt"] == "ordinary"));
            assert!(
                items
                    .iter()
                    .any(|item| item["kind"] == "turn_finished" && item["turn_id"] == *turn)
            );
        }
        if provider == "codex" {
            assert!(items.iter().any(|item| item["kind"] == "native_response"
                && item["data"]["method"] == "thread/start"
                && item["data"]["response"]["thread"]["fixtureThread"]["unknown"][1]["keep"]
                    == true));
            assert!(items.iter().any(|item| item["kind"] == "native_response"
                && item["data"]["method"] == "turn/start"
                && item["data"]["response"]["fixtureResponse"]["unknown"][1]["keep"] == true));
        } else {
            assert!(items.iter().any(|item| item["kind"] == "native_summary"));
            assert!(items.iter().any(|item| item["kind"] == "native_snapshot"
                && item["data"]["event"]["fixtureInit"]["unknown"] == true));
        }
        assert_eq!(
            client.call(
                "session_read",
                json!({"session_id":session,"after":after + 1})
            )["isError"],
            true
        );
        assert_eq!(
            client.call("session_read", json!({"session_id":session,"limit":257}))["isError"],
            true
        );
        let pushed: Vec<_> = client
            .notifications
            .iter()
            .filter(|m| {
                m["method"] == "notifications/due/session_item"
                    && m["params"]["session_id"] == session
            })
            .collect();
        assert!(pushed.windows(2).all(
            |pair| pair[0]["params"]["cursor"].as_u64() < pair[1]["params"]["cursor"].as_u64()
        ));
        client.close(&session);
        client.eof();
    }
}

#[test]
fn pushed_interactions_can_be_replied_to_without_polling() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open_stream(provider);
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
        client.ok(
            "interaction_reply",
            json!({"session_id":session,"turn_id":turn,"request_id":request,"response":response}),
        );
        let finished = client.pushed_finish(&session, &turn);
        assert_eq!(finished["data"]["text"], "permission denied");
        let replay = client.ok("session_read", json!({"session_id":session}));
        assert!(
            replay["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "interaction_resolved"
                    && item["data"]["request_id"] == request)
        );
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
        assert!(!client.dir.path().join("never-created").exists());
        client.close(&session);
        client.eof();
    }
}

#[test]
fn pushes_codex_completion_that_arrives_before_the_start_response() {
    let mut client = Client::new(true);
    let session = client.open_stream("codex");
    for number in 1..=2 {
        let turn = client.start(&session, "ordinary");
        let finished = client.pushed_finish(&session, &turn);
        assert_eq!(finished["data"]["status"], "succeeded");
        assert_eq!(finished["data"]["text"], format!("turn {number}"));
        assert!(
            client
                .notifications
                .iter()
                .any(|message| message["params"]["kind"] == "native"
                    && message["params"]["data"]["method"] == "turn/completed")
        );
    }
    client.close(&session);
    client.eof();
}

#[test]
fn pushes_codex_items_outside_a_turn() {
    let mut client = Client::new(false);
    let session = client.open_stream("codex");
    let idle = client.pushed_until(|item| {
        item["session_id"] == session
            && item["kind"] == "native"
            && item["data"]["method"] == "fixture/idle"
    });
    assert_eq!(idle["turn_id"], Value::Null);
    assert_eq!(
        idle["data"]["params"]["future"],
        json!({"nested":[null,{"keep":true}]})
    );
    let replay = client.ok("session_read", json!({"session_id":session}));
    assert!(
        replay["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["cursor"] == idle["cursor"] && item["data"] == idle["data"])
    );
    client.close(&session);
    client.eof();
}
