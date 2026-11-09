#![cfg(unix)]

use std::{
    collections::VecDeque, fs, os::unix::fs::PermissionsExt, path::Path, process::Stdio,
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

const DEADLINE: Duration = Duration::from_secs(15);

struct Peer {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
    inbox: VecDeque<Value>,
    next_id: u64,
}

impl Peer {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_due-rpc"))
            .current_dir(root)
            .env("DUE_RPC_CLAUDE_BINARY", fake_provider(&root.join("claude")))
            .env("DUE_RPC_CODEX_BINARY", fake_provider(&root.join("codex")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            inbox: VecDeque::new(),
            next_id: 1,
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        timeout(
            DEADLINE,
            self.input
                .as_mut()
                .unwrap()
                .write_all(format!("{request}\n").as_bytes()),
        )
        .await
        .unwrap()
        .unwrap();
        id
    }

    async fn matching(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.inbox.iter().position(&matches) {
            return self.inbox.remove(index).unwrap();
        }
        timeout(DEADLINE, async {
            loop {
                let line = self
                    .output
                    .next_line()
                    .await
                    .unwrap()
                    .expect("unexpected RPC EOF");
                let frame: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("invalid RPC frame: {error}: {line}"));
                assert_eq!(frame["jsonrpc"], "2.0");
                if matches(&frame) {
                    return frame;
                }
                self.inbox.push_back(frame);
            }
        })
        .await
        .expect("RPC response timed out")
    }

    async fn response(&mut self, id: u64) -> Value {
        self.matching(|frame| frame["id"] == id).await
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params).await;
        let response = self.response(id).await;
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    async fn subscribe(&mut self, session: &Value, after: u64) -> Subscription {
        let response = self
            .call(
                "session.subscribe",
                json!({"session":session,"after":after}),
            )
            .await;
        assert_eq!(response["cursor"], after);
        assert_eq!(response["session"], *session);
        let id = response["subscription"].as_u64().unwrap();
        assert!(
            !self
                .inbox
                .iter()
                .any(|frame| frame["params"]["subscription"] == id)
        );
        Subscription {
            id,
            session: session.clone(),
            cursor: after,
        }
    }

    async fn item(&mut self, subscription: &mut Subscription) -> Value {
        let frame = self
            .matching(|frame| frame["params"]["subscription"] == subscription.id)
            .await;
        assert_eq!(frame["method"], "session.item", "{frame}");
        assert!(frame.get("id").is_none());
        let item = frame["params"]["item"].clone();
        subscription.cursor += 1;
        assert_eq!(item["cursor"], subscription.cursor);
        assert_eq!(item["session"], subscription.session);
        item
    }

    async fn until(&mut self, subscription: &mut Subscription, run: &Value, kind: &str) -> Value {
        timeout(DEADLINE, async {
            loop {
                let item = self.item(subscription).await;
                if item["run"] == *run && item["kind"] == kind {
                    return item;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("did not see {kind} for {run}"))
    }
}

struct Subscription {
    id: u64,
    session: Value,
    cursor: u64,
}

fn fake_provider(directory: &Path) -> std::path::PathBuf {
    fs::create_dir(directory).unwrap();
    fs::write(
        directory.join("native"),
        include_bytes!("../../due-core/tests/fixtures/session-agent.sh"),
    )
    .unwrap();
    let executable = directory.join("provider");
    fs::write(
        &executable,
        r#"#!/bin/sh
export DUE_SESSION_FIXTURE_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)"
exec /bin/bash "$DUE_SESSION_FIXTURE_DIR/native" "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    executable
}

#[tokio::test]
async fn streams_approvals_and_replays_both_providers_across_follow_ups() {
    let workspace = tempfile::tempdir().unwrap();
    let mut peer = Peer::start(workspace.path());
    assert_eq!(
        peer.call("server.info", json!({})).await["protocol_version"],
        1
    );
    let claude = peer
        .call("run.spawn", json!({"provider":"claude","prompt":"ask"}))
        .await;
    let codex = peer
        .call("run.spawn", json!({"provider":"codex","prompt":"ask"}))
        .await;
    assert_ne!(claude["session"], codex["session"]);

    let mut subscriptions = [
        peer.subscribe(&claude["session"], 0).await,
        peer.subscribe(&codex["session"], 0).await,
    ];
    let decisions = [json!({"behavior":"allow"}), json!({"decision":"decline"})];
    let initial = [&claude, &codex];
    let mut tokens = Vec::new();
    for (run, subscription) in initial.iter().zip(&mut subscriptions) {
        let item = peer.until(subscription, &run["run"], "interaction").await;
        assert!(item["data"]["request"].is_object());
        assert!(item["interaction"].is_object());
        let poll = peer
            .call("run.wait", json!({"runs":[run["run"]],"timeout_ms":0}))
            .await;
        assert_eq!(poll["condition_met"], false);
        let stopped = peer
            .call(
                "session.unsubscribe",
                json!({"subscription":subscription.id}),
            )
            .await;
        assert_eq!(stopped["unsubscribed"], true);
        tokens.push(item["interaction"].clone());
    }

    // This wait remains outstanding while the same connection answers approvals.
    let waiting = peer
        .request(
            "run.wait",
            json!({
                "runs":[claude["run"],codex["run"]],"timeout_ms":10000
            }),
        )
        .await;
    for (token, decision) in tokens.iter().zip(&decisions) {
        let receipt = peer
            .call(
                "interaction.respond",
                json!({"interaction":token,"response":decision}),
            )
            .await;
        assert_eq!(receipt["interaction"], *token);
    }
    let finished = peer.response(waiting).await["result"].clone();
    assert_eq!(finished["condition_met"], true, "{finished}");
    assert_eq!(finished["runs"][0]["result"]["text"], "permission allowed");
    assert_eq!(finished["runs"][1]["result"]["text"], "permission denied");

    for index in 0..2 {
        let old = &subscriptions[index];
        let page = peer
            .call(
                "session.read",
                json!({"session":old.session,"after":old.cursor}),
            )
            .await;
        let expected = page["items"].as_array().unwrap();
        assert!(!expected.is_empty());
        let mut resumed = peer.subscribe(&old.session, old.cursor).await;
        assert_ne!(resumed.id, old.id);
        for replayed in expected {
            assert_eq!(peer.item(&mut resumed).await, *replayed);
        }
        assert_eq!(resumed.cursor, page["cursor"]);
        subscriptions[index] = resumed;

        let stale = peer
            .request(
                "interaction.respond",
                json!({
                    "interaction":tokens[index],"response":decisions[index]
                }),
            )
            .await;
        let stale = peer.response(stale).await;
        assert_eq!(stale["error"]["data"]["kind"], "unknown_interaction");
    }

    for (run, subscription) in initial.iter().zip(&mut subscriptions) {
        let next = peer
            .call(
                "session.follow_up",
                json!({"session":run["session"],"prompt":"second"}),
            )
            .await;
        assert_eq!(next["session"], run["session"]);
        assert_ne!(next["run"], run["run"]);
        let admitted = peer.until(subscription, &next["run"], "run_admitted").await;
        assert_eq!(admitted["run"], next["run"]);
        let final_item = peer.until(subscription, &next["run"], "run_finished").await;
        assert_eq!(final_item["data"]["text"], "turn 2", "{final_item}");
    }

    let held = peer
        .call(
            "session.follow_up",
            json!({"session":codex["session"],"prompt":"ask"}),
        )
        .await;
    peer.until(&mut subscriptions[1], &held["run"], "interaction")
        .await;
    let wait = peer
        .request("run.wait", json!({"runs":[held["run"]],"timeout_ms":10000}))
        .await;
    drop(peer.input.take());
    let settled = peer.response(wait).await;
    assert_eq!(
        settled["result"]["runs"][0]["result"]["status"], "cancelled",
        "{settled}"
    );
    assert!(
        timeout(DEADLINE, peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    for provider in ["claude", "codex"] {
        assert!(workspace.path().join(provider).join("exited").is_file());
    }
}

#[tokio::test]
async fn broken_stdout_settles_a_worker_even_with_stdin_open() {
    let workspace = tempfile::tempdir().unwrap();
    let mut peer = Peer::start(workspace.path());
    let run = peer
        .call("run.spawn", json!({"provider":"claude","prompt":"ask"}))
        .await;
    let mut subscription = peer.subscribe(&run["session"], 0).await;
    peer.until(&mut subscription, &run["run"], "interaction")
        .await;

    let Peer {
        mut child,
        input,
        output,
        ..
    } = peer;
    drop(output);
    let mut input = input.unwrap();
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"server.info\"}\n")
        .await
        .unwrap();
    assert!(
        !timeout(DEADLINE, child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(workspace.path().join("claude").join("exited").is_file());
    drop(input);
}
