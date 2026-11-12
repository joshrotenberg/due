#![cfg(all(feature = "rpc", unix))]

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

#[path = "rpc/bounds_review.rs"]
mod bounds_review;
#[path = "rpc/close_review.rs"]
mod close_review;
#[path = "rpc/codex_partial.rs"]
mod codex_partial;
#[path = "rpc/numeric_params.rs"]
mod numeric_params;
#[path = "rpc/profiles.rs"]
mod profiles;
#[path = "rpc/transport_review.rs"]
mod transport_review;

// A concurrent child launch can temporarily retain a caller's pipe read end
// until exec closes inherited descriptors. Serialize launches with read-end
// teardown so a post-disconnect probe cannot write through that extra reader.
static PROCESS_SPAWN: Mutex<()> = Mutex::new(());

fn spawn_rpc(command: &mut Command) -> Child {
    let _guard = PROCESS_SPAWN.lock().unwrap();
    command.spawn().unwrap()
}

struct Client {
    dir: TempDir,
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    notifications: Vec<Value>,
    responses: BTreeMap<String, Value>,
    next_id: u64,
    close_stdout: Arc<AtomicBool>,
    // Bounded notes from the stdout reader thread, for failure reports only.
    reader_trace: Arc<Mutex<Vec<String>>>,
}

impl Client {
    fn new(early: bool) -> Self {
        Self::spawn(early, false)
    }

    /// `log_stderr` sends Due's stderr to `stderr.log` in the fixture directory
    /// so a failing test can include it; otherwise stderr is inherited.
    fn spawn(early: bool, log_stderr: bool) -> Self {
        Self::with_stderr(early, |directory| {
            if log_stderr {
                Stdio::from(fs::File::create(directory.join("stderr.log")).unwrap())
            } else {
                Stdio::inherit()
            }
        })
    }

    fn with_stderr(early: bool, stderr: impl FnOnce(&std::path::Path) -> Stdio) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("provider");
        fs::write(
            &binary,
            include_bytes!("../../due-core/tests/fixtures/session-agent.sh"),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = spawn_rpc(
            Command::new(env!("CARGO_BIN_EXE_due"))
                .arg("rpc")
                .env("DUE_SESSION_FIXTURE_DIR", dir.path())
                .env("DUE_SESSION_EARLY", if early { "1" } else { "0" })
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(stderr(dir.path())),
        );
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        let close_stdout = Arc::new(AtomicBool::new(false));
        let reader_close = close_stdout.clone();
        let reader_trace = Arc::new(Mutex::new(Vec::new()));
        let trace = reader_trace.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let mut lines_read = 0_u32;
            let mut reader = BufReader::new(stdout);
            for line in reader.by_ref().lines() {
                lines_read += 1;
                if reader_close.load(Ordering::Acquire) {
                    // Record which line woke the reader; a pushed item and a
                    // reply lead to different timelines.
                    let kind = line
                        .as_deref()
                        .ok()
                        .and_then(|line| serde_json::from_str::<Value>(line).ok())
                        .map(|value| {
                            format!(
                                "id={} method={}",
                                value.get("id").unwrap_or(&Value::Null),
                                value.get("method").unwrap_or(&Value::Null)
                            )
                        })
                        .unwrap_or_else(|| "unparsed".into());
                    trace.lock().unwrap().push(format!(
                        "{}ms reader requested stdout close on line {lines_read} ({kind})",
                        started.elapsed().as_millis()
                    ));
                    break;
                }
                let line = line.unwrap();
                let message = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("non-protocol stdout {line:?}: {error}"));
                if tx.send(message).is_err() {
                    break;
                }
            }
            // Complete any in-flight child startup before closing this pipe.
            // The sender's disconnect is the caller's post-close write barrier.
            let close_guard = PROCESS_SPAWN.lock().unwrap();
            drop(reader);
            trace.lock().unwrap().push(format!(
                "{}ms stdout closed after child launches settled",
                started.elapsed().as_millis()
            ));
            drop(tx);
            drop(close_guard);
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
            reader_trace,
        }
    }

    /// Evidence for a hang report: provider, test-side timeline, reader notes,
    /// Due's liveness and the tail of its stderr. Bounded in size.
    fn evidence(&mut self, provider: &str, timeline: &[String]) -> String {
        let stderr = fs::read(self.dir.path().join("stderr.log")).unwrap_or_default();
        let tail = &stderr[stderr.len().saturating_sub(4096)..];
        format!(
            "due_pid={} provider={provider}\ntimeline={timeline:?}\nreader={:?}\ndue_status={:?}\n\
             native_exited={} native_pid_recorded={}\ndue_stderr_tail={:?}",
            self.child.id(),
            self.reader_trace.lock().unwrap(),
            self.child.try_wait(),
            self.dir.path().join("exited").exists(),
            self.dir.path().join("pid").exists(),
            String::from_utf8_lossy(tail),
        )
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
fn claude_interrupted_partial_text_is_preserved_once_and_scoped_to_turn() {
    check_claude_interrupted_partial_text(false);
}

#[test]
fn claude_interrupted_partial_text_prefers_nonempty_terminal_answer() {
    check_claude_interrupted_partial_text(true);
}

fn check_claude_interrupted_partial_text(terminal_answer: bool) {
    let mut client = Client::new(false);
    let session = client.open("claude", true, None);
    let mut outcomes = Vec::new();
    for number in 1..=2 {
        let partial = format!("Partial answer for turn {number}.\nStill incomplete.");
        let terminal = if terminal_answer {
            format!("Authoritative terminal answer for turn {number}.")
        } else {
            String::new()
        };
        let turn = client.start(
            &session,
            if terminal_answer {
                "interrupted-partial-terminal"
            } else {
                "interrupted-partial-empty"
            },
        );
        // Synchronize on the final text delta, not a sleep or the turn/start reply.
        client.pushed_until(|item| {
            item["session_id"] == session
                && item["turn_id"] == turn
                && item["kind"] == "native"
                && item["data"]["event"]["type"] == "stream_event"
                && item["data"]["event"]["event"]["delta"]["text"] == "Still incomplete."
        });
        assert_eq!(
            client.ok(
                "turn.interrupt",
                json!({"session_id":session,"turn_id":turn}),
            )["interrupt_requested"],
            true
        );
        let outcome = client.finished(&session, &turn);
        assert_eq!(outcome["status"], "cancelled", "{outcome}");
        assert_eq!(outcome["error"]["kind"], "cancelled");
        assert_eq!(outcome["run_id"], turn);
        assert_eq!(outcome["session_id"], "native-claude");
        assert_eq!(outcome["usage"]["input_tokens"], 10);
        assert_eq!(outcome["usage"]["output_tokens"], 0);
        assert!(outcome["usage"]["total_tokens"].is_null());
        assert_eq!(outcome["cost_usd"], 0.01);
        let read = client.ok("turn.read", json!({"session_id":session,"turn_id":turn}));
        assert_eq!(read["state"], "finished");
        assert_eq!(read["result"], outcome);

        let replay = client.ok("session.read", json!({"session_id":session}));
        assert_eq!(replay["items_lost"], false);
        let events: Vec<_> = replay["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["turn_id"] == turn && item["kind"] == "native")
            .map(|item| &item["data"]["event"])
            .collect();
        let deltas: Vec<_> = events
            .iter()
            .filter(|event| event["event"]["delta"]["type"] == "text_delta")
            .map(|event| event["event"]["delta"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas.concat(), partial);
        let assistant = events
            .iter()
            .find(|event| event["type"] == "assistant")
            .expect("aborted assistant snapshot must be retained");
        assert_eq!(assistant["aborted"], true);
        assert_eq!(assistant["message"]["content"][0]["type"], "thinking");
        assert_eq!(assistant["message"]["content"][1]["type"], "tool_use");
        assert_eq!(assistant["message"]["content"][2]["text"], partial);
        let native_result = events
            .iter()
            .find(|event| event["type"] == "result")
            .expect("native terminal result must be retained");
        assert_eq!(native_result["subtype"], "success");
        assert_eq!(native_result["is_error"], false);
        assert_eq!(native_result["terminal_reason"], "aborted_streaming");
        assert_eq!(native_result["usage"]["output_tokens"], 0);
        assert_eq!(native_result["result"], terminal);
        outcomes.push((outcome, if terminal_answer { terminal } else { partial }));
    }
    let next = client.start(&session, "ordinary");
    let completed = client.finished(&session, &next);
    assert_eq!(completed["status"], "succeeded");
    assert_eq!(completed["text"], "turn 3");
    client.close(&session);
    client.eof();

    // Exercise the follow-up and cleanup even on the unfixed adapter. Exact text
    // rejects duplicated deltas/snapshots, tool/thinking text and prior-turn text.
    for (outcome, expected) in outcomes {
        assert_eq!(outcome["text"], expected, "{outcome}");
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
fn concurrent_workers_settle_independently_on_eof_and_explicit_close() {
    for (provider, sibling_provider) in [("claude", "codex"), ("codex", "claude")] {
        let mut client = Client::new(false);
        let mut sibling = Client::new(false);
        let session = client.open(provider, true, None);
        let sibling_session = sibling.open(sibling_provider, true, None);
        let turn = client.start(&session, "hold");
        let sibling_turn = sibling.start(&sibling_session, "hold");
        client.ready(provider, &session, &turn);
        sibling.ready(sibling_provider, &sibling_session, &sibling_turn);

        client.eof();
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        client
            .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_closed");
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived EOF"
        );

        let snapshot = sibling.ok(
            "turn.read",
            json!({"session_id":sibling_session,"turn_id":sibling_turn}),
        );
        assert_eq!(snapshot["state"], "running", "{snapshot}");
        sibling.close(&sibling_session);
        assert_eq!(
            sibling.finished(&sibling_session, &sibling_turn)["status"],
            "cancelled"
        );
        sibling.eof();
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
fn session_open_rejects_resume_ids_parsed_as_native_options() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        for resume in ["--dangerously-skip-permissions", "-c", "native\nid"] {
            let options = json!({
                "provider":provider,"binary":client.dir.path().join("provider"),
                "cwd":client.dir.path(),"resume":resume,
            });
            let reply = client.call("session.open", options);
            assert_eq!(
                reply["error"]["code"], -32000,
                "{provider} {resume:?}: {reply}"
            );
            assert!(
                !client.dir.path().join("pid").exists(),
                "{provider} {resume:?}"
            );
        }
        let session = client.open(provider, false, None);
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
fn envelope_diagnostics_distinguish_batches_versions_and_malformed_json() {
    let mut client = Client::new(false);
    for (line, code, message) in [
        ("{broken", -32700, "Parse error"),
        (
            "[]",
            -32600,
            "Invalid Request (one object per line; no batches)",
        ),
        (
            r#"[{"jsonrpc":"2.0","id":1,"method":"missing"}]"#,
            -32600,
            "Invalid Request (one object per line; no batches)",
        ),
        (
            "null",
            -32600,
            "Invalid Request (one object per line; no batches)",
        ),
        (
            r#"{"id":1,"method":"missing"}"#,
            -32600,
            "Invalid Request (jsonrpc must be \"2.0\")",
        ),
        (
            r#"{"jsonrpc":"1.0","id":1,"method":"missing"}"#,
            -32600,
            "Invalid Request (jsonrpc must be \"2.0\")",
        ),
        (
            r#"{"jsonrpc":2,"method":"missing"}"#,
            -32600,
            "Invalid Request (jsonrpc must be \"2.0\")",
        ),
        (
            r#"{"jsonrpc":"2.0","method":42}"#,
            -32600,
            "Invalid Request (invalid JSON-RPC envelope)",
        ),
    ] {
        client.raw(format!("{line}\n").as_bytes());
        assert_eq!(
            client.response(&Value::Null, Duration::from_secs(3)),
            json!({"jsonrpc":"2.0","id":null,"error":{"code":code,"message":message}}),
            "{line}"
        );
    }
    // Valid envelopes without IDs remain silent, including invalid params.
    client.write(json!({"jsonrpc":"2.0","method":"missing"}));
    client.write(json!({"jsonrpc":"2.0","method":"session.open","params":[]}));
    assert_eq!(client.call("missing", json!({}))["error"]["code"], -32601);
    client.eof();
    assert!(client.messages.iter().next().is_none());
    assert!(
        client.responses.is_empty(),
        "notification received a response"
    );
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
        let started = Instant::now();
        let mut timeline: Vec<String> = Vec::new();
        let note = |timeline: &mut Vec<String>, event: String| {
            timeline.push(format!("{}ms {event}", started.elapsed().as_millis()))
        };
        let mut client = Client::spawn(false, true);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        note(&mut timeline, "native turn ready".into());
        // Wake the reader with one harmless reply, then let it drop the owned
        // stdout pipe. No cross-thread raw descriptor closing is needed.
        client.close_stdout.store(true, Ordering::Release);
        client.request("missing", json!({}));
        note(&mut timeline, "first missing request sent".into());
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut delivered = 0_u32;
        loop {
            match client
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(_) => delivered += 1,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => {
                    note(&mut timeline, format!("reader wait failed: {error}"));
                    let evidence = client.evidence(provider, &timeline);
                    panic!("stdout reader did not close: {error}\n{evidence}");
                }
            }
        }
        // Whether Due had already seen the broken pipe before the second
        // request distinguishes "reply raced the reader" from "no EPIPE seen".
        note(
            &mut timeline,
            format!(
                "reader disconnected after {delivered} messages; due exited: {:?}",
                client.child.try_wait().unwrap()
            ),
        );
        client.request("missing", json!({}));
        note(&mut timeline, "second missing request sent".into());
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = client.child.try_wait().unwrap() {
                note(&mut timeline, format!("due exited: {status}"));
                assert!(
                    !status.success(),
                    "broken stdout must fail the connection\n{}",
                    client.evidence(provider, &timeline)
                );
                break;
            }
            if Instant::now() >= deadline {
                note(&mut timeline, "deadline reached".into());
                let evidence = client.evidence(provider, &timeline);
                panic!(
                    "Due survived broken stdout with stdin held open; native exit observed: {}\n{evidence}",
                    client.dir.path().join("exited").exists()
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(client.stdin.is_some());
        assert!(
            client.dir.path().join("exited").exists(),
            "native process survived broken stdout\n{}",
            client.evidence(provider, &timeline)
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

    let mut child = OwnedChild(spawn_rpc(
        Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    ));
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

#[test]
fn codex_startup_interrupt_waits_for_acceptance_and_does_not_leak_to_next_turn() {
    for prompt in ["startup-hold", "startup-fail", "startup-complete"] {
        let mut client = Client::new(false);
        let session = client.open("codex", true, None);
        // Due start completes while the native start response is file-gated.
        let turn = client.start(&session, prompt);
        if prompt == "startup-complete" {
            // Observe completion while native identity is still withheld. A wire
            // write alone is insufficient: the owner's two input queues race.
            client
                .pushed_until(|i| i["kind"] == "native" && i["data"]["method"] == "turn/completed");
        }
        let interrupt = client.request(
            "turn.interrupt",
            json!({"session_id":session,"turn_id":turn}),
        );
        client.pushed_until(|i| i["kind"] == "interrupt_pending" && i["turn_id"] == turn);
        assert!(!client.responses.contains_key(&interrupt.to_string()));
        let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
        assert!(!input.contains("turn/interrupt"), "{input}");
        fs::write(client.dir.path().join("release-start"), "release").unwrap();
        let response = client.response(&interrupt, Duration::from_secs(5));
        let outcome = client.finished(&session, &turn);
        if prompt == "startup-hold" {
            assert_eq!(
                response["result"]["interrupt_requested"], true,
                "{response}"
            );
            assert_eq!(outcome["status"], "cancelled", "{outcome}");
        } else {
            assert_eq!(response["error"]["code"], -32000, "{response}");
            assert_eq!(
                outcome["status"],
                if prompt == "startup-fail" {
                    "failed"
                } else {
                    "succeeded"
                }
            );
        }
        if prompt != "startup-fail" {
            let next = client.start(&session, "followup");
            assert_eq!(client.finished(&session, &next)["status"], "succeeded");
        }
        client.close(&session);
        client.eof();
        let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
        let controls: Vec<Value> = input
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .filter(|v: &Value| v["method"] == "turn/interrupt")
            .collect();
        assert_eq!(
            controls.len(),
            usize::from(prompt == "startup-hold"),
            "{input}"
        );
        if let Some(control) = controls.first() {
            assert_eq!(control["params"]["turnId"], "turn-1");
        }
        assert!(
            !client.responses.contains_key(&interrupt.to_string()),
            "duplicate interrupt response"
        );
    }
}

#[test]
fn codex_startup_interrupt_does_not_block_read_close_or_eof() {
    for eof in [false, true] {
        let mut client = Client::new(false);
        let session = client.open("codex", true, None);
        let turn = client.start(&session, "startup-hold");
        let interrupt = client.request(
            "turn.interrupt",
            json!({"session_id":session,"turn_id":turn}),
        );
        client.pushed_until(|i| i["kind"] == "interrupt_pending" && i["turn_id"] == turn);
        let read = client.request("session.read", json!({"session_id":session}));
        assert!(
            client
                .response(&read, Duration::from_secs(2))
                .get("result")
                .is_some()
        );
        let duplicate = client.request(
            "turn.interrupt",
            json!({"session_id":session,"turn_id":turn}),
        );
        let rejected = client.response(&duplicate, Duration::from_secs(2));
        assert!(
            rejected["error"]["message"]
                .as_str()
                .unwrap()
                .contains("already pending"),
            "{rejected}"
        );
        let close = if eof {
            client.stdin.take();
            None
        } else {
            Some(client.request("session.close", json!({"session_id":session})))
        };
        // Closing must reach the owner and settle interruption while native
        // turn/start is still withheld, not after the 30-second control timeout.
        let response = client.response(&interrupt, Duration::from_secs(2));
        assert_eq!(response["error"]["code"], -32000, "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("closed"),
            "{response}"
        );
        fs::write(client.dir.path().join("release-start"), "release").unwrap();
        if let Some(close) = close {
            assert_eq!(
                client.response(&close, Duration::from_secs(10))["result"]["closed"],
                true
            );
        }
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        client.eof();
        let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
        assert!(!input.contains("turn/interrupt"), "{input}");
    }
}

#[test]
fn codex_pending_interrupt_allows_sibling_deadline_permission_reply_and_followup() {
    let mut client = Client::new(false);
    let session = client.open("codex", true, None);
    let turn = client.start(&session, "startup-ask");
    let interrupt = client.request(
        "turn.interrupt",
        json!({"session_id":session,"turn_id":turn}),
    );
    client.pushed_until(|i| i["kind"] == "interrupt_pending" && i["turn_id"] == turn);
    fs::write(client.dir.path().join("release-start"), "release").unwrap();
    let interaction = client.pushed_until(|i| i["kind"] == "interaction" && i["turn_id"] == turn);
    // Fake native interrupt deliberately withholds acceptance until permission
    // is answered. Holding the manager lock here deadlocks both operations.
    client.pushed_until(|i| {
        i["kind"] == "native" && i["data"]["method"] == "fixture/interrupt-waiting"
    });
    let mut sibling = Client::new(false);
    let sibling_session = sibling.open("claude", true, Some("200ms"));
    let sibling_turn = sibling.start(&sibling_session, "hold");
    let expired = sibling.finished(&sibling_session, &sibling_turn);
    assert_eq!(expired["status"], "timed_out", "{expired}");
    assert!(sibling.dir.path().join("exited").is_file());
    sibling.call("session.close", json!({"session_id":sibling_session}));
    sibling.eof();

    let read = client.request("session.read", json!({"session_id":session}));
    assert!(
        client
            .response(&read, Duration::from_secs(2))
            .get("result")
            .is_some()
    );
    let reply = client.request(
        "interaction.reply",
        json!({"session_id":session,"turn_id":turn,
        "request_id":interaction["data"]["request_id"],"response":{"decision":"cancel"}}),
    );
    assert_eq!(
        client.response(&reply, Duration::from_secs(2))["result"]["replied"],
        true
    );
    assert_eq!(
        client.response(&interrupt, Duration::from_secs(2))["result"]["interrupt_requested"],
        true
    );
    assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
    let next = client.start(&session, "followup");
    assert_eq!(client.finished(&session, &next)["status"], "succeeded");
    assert!(
        client
            .call(
                "turn.interrupt",
                json!({"session_id":session,"turn_id":turn})
            )
            .get("error")
            .is_some()
    );
    client.close(&session);
    client.eof();
}

#[test]
fn codex_completion_before_interrupt_ack_keeps_original_response_and_identity() {
    for mode in ["late-ack", "late-error", "close"] {
        let mut client = Client::new(false);
        let session = client.open("codex", true, None);
        let first = client.start(&session, if mode == "close" { "late-ack" } else { mode });
        let interrupt = client.request(
            "turn.interrupt",
            json!({"session_id":session,"turn_id":first}),
        );
        assert_eq!(client.finished(&session, &first)["status"], "cancelled");
        // The completed turn must not own/cancel the native request waiter.
        // A second live turn also tests the session-wide one-control bound.
        let next = client.start(&session, "ask");
        let permission =
            client.pushed_until(|i| i["kind"] == "interaction" && i["turn_id"] == next);
        let read = client.request("session.read", json!({"session_id":session}));
        assert!(
            client
                .response(&read, Duration::from_secs(2))
                .get("result")
                .is_some()
        );
        assert!(
            !client.responses.contains_key(&interrupt.to_string()),
            "interrupt settled before gated acknowledgement"
        );
        let second = client.request(
            "turn.interrupt",
            json!({"session_id":session,"turn_id":next}),
        );
        let rejected = client.response(&second, Duration::from_secs(2));
        assert!(
            rejected["error"]["message"]
                .as_str()
                .unwrap()
                .contains("already pending"),
            "{rejected}"
        );
        if mode == "close" {
            let close = client.request("session.close", json!({"session_id":session}));
            let response = client.response(&interrupt, Duration::from_secs(2));
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("closed"),
                "{response}"
            );
            assert_eq!(
                client.response(&close, Duration::from_secs(10))["result"]["closed"],
                true
            );
        } else {
            fs::write(client.dir.path().join("release-ack"), "release").unwrap();
            // A native permission response wakes the fixture's blocking input
            // loop. It finishes next before releasing the original control ack.
            client.ok(
                "interaction.reply",
                json!({"session_id":session,"turn_id":next,
                "request_id":permission["data"]["request_id"],"response":{"decision":"cancel"}}),
            );
            assert_eq!(client.finished(&session, &next)["status"], "succeeded");
            let response = client.response(&interrupt, Duration::from_secs(2));
            if mode == "late-ack" {
                assert_eq!(
                    response["result"]["interrupt_requested"], true,
                    "{response}"
                );
                assert_eq!(response["result"]["turn_id"], first);
                client
                    .pushed_until(|i| i["kind"] == "interrupt_requested" && i["turn_id"] == first);
            } else {
                assert!(
                    response["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("fixture late interrupt rejection"),
                    "{response}"
                );
                client.pushed_until(|i| {
                    i["kind"] == "native_response"
                        && i["turn_id"] == first
                        && i["data"]["method"] == "turn/interrupt"
                        && i["data"]["error"].is_object()
                });
            }
            // The completed follow-up retains its successful result after late ack.
            let state = client.ok("turn.read", json!({"session_id":session,"turn_id":next}));
            assert_eq!(state["result"]["status"], "succeeded", "{state}");
            let last = client.start(&session, "followup");
            assert_eq!(client.finished(&session, &last)["status"], "succeeded");
            client.close(&session);
        }
        client.eof();
        assert!(
            !client.responses.contains_key(&interrupt.to_string()),
            "duplicate original reply"
        );
        let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
        let controls: Vec<Value> = input
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .filter(|v: &Value| v["method"] == "turn/interrupt")
            .collect();
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0]["params"]["turnId"], "turn-1");
    }
}

#[test]
fn concurrent_callers_settle_after_broken_stdout_with_input_open() {
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..8 {
                    broken_stdout_stops_due_and_native_process_with_stdin_still_open();
                }
            });
        }
    });
}
