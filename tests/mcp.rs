#![cfg(all(unix, feature = "mcp-stdio"))]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{sleep, timeout},
};

const DEADLINE: Duration = Duration::from_secs(15);

struct Peer {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Peer {
    fn start(root: &Path, catalog: Option<&Path>) -> Self {
        let cwd = root.join("worker-cwd");
        fs::create_dir(&cwd).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_due"));
        command
            .args(["mcp", "--cwd"])
            .arg(&cwd)
            .current_dir(root)
            .env("DUE_MCP_CLAUDE_BINARY", fake_provider(&root.join("claude")))
            .env("DUE_MCP_CODEX_BINARY", fake_provider(&root.join("codex")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = catalog {
            command.arg("--profiles").arg(path);
        }
        let mut child = command.spawn().unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            next_id: 1,
        }
    }

    async fn initialize(&mut self) {
        let initialized = self
            .call(
                "initialize",
                json!({
                    "protocolVersion":"2025-11-25","capabilities":{},
                    "clientInfo":{"name":"due-mcp-test","version":"0.1"}
                }),
            )
            .await;
        assert_eq!(initialized["protocolVersion"], "2025-11-25");
        assert_eq!(initialized["serverInfo"]["name"], "due-mcp");
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
    }

    async fn send(&mut self, message: Value) {
        timeout(
            DEADLINE,
            self.input.write_all(format!("{message}\n").as_bytes()),
        )
        .await
        .expect("writing MCP request timed out")
        .unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        id
    }

    async fn receive(&mut self) -> Value {
        let line = timeout(DEADLINE, self.output.next_line())
            .await
            .expect("reading MCP response timed out")
            .unwrap()
            .expect("unexpected MCP EOF");
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("invalid MCP frame: {error}: {line}"))
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params).await;
        let response = self.receive().await;
        assert_eq!(response["id"], id, "{response}");
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let result = self
            .call("tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        assert_ne!(result["isError"], true, "{result}");
        let value = result["structuredContent"].clone();
        assert!(value.is_object(), "{result}");
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(value, text);
        value
    }

    async fn awaiting_input(&mut self, run: &Value) -> Value {
        timeout(DEADLINE, async {
            loop {
                let snapshot = self.tool("inspect", json!({"run":run})).await;
                assert_ne!(snapshot["state"], "finished", "{snapshot}");
                if snapshot["state"] == "awaiting_input" {
                    return snapshot;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("provider did not request approval")
    }

    async fn read_all(&mut self, session: &Value, after: &mut u64) -> Vec<Value> {
        let mut items = Vec::new();
        loop {
            let page = self
                .tool(
                    "read_items",
                    json!({"session":session,"after":after,"limit":2}),
                )
                .await;
            let page_items = page["items"].as_array().unwrap();
            assert!(page_items.len() <= 2);
            for item in page_items {
                assert_eq!(item["session"], *session);
                *after += 1;
                assert_eq!(item["cursor"], *after);
                items.push(item.clone());
            }
            assert_eq!(page["cursor"], *after);
            if page["cursor"] == page["latest_cursor"] {
                assert_eq!(page["closed"], false);
                return items;
            }
            assert!(!page_items.is_empty(), "{page}");
        }
    }
}

fn fake_provider(directory: &Path) -> std::path::PathBuf {
    fs::create_dir(directory).unwrap();
    fs::write(
        directory.join("native"),
        include_bytes!("../crates/due-core/tests/fixtures/session-agent.sh"),
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
async fn stdio_drives_two_resident_sessions_and_settles_on_eof() {
    let workspace = tempfile::tempdir().unwrap();
    let mut peer = Peer::start(workspace.path(), None);
    peer.initialize().await;
    let listed = peer.call("tools/list", json!({})).await;
    let spawn = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "spawn")
        .unwrap();
    assert_eq!(
        spawn["inputSchema"]["required"],
        json!(["prompt", "provider"])
    );
    assert!(spawn["inputSchema"]["properties"].get("profile").is_none());
    let mut tools: Vec<_> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    tools.sort_unstable();
    assert_eq!(
        tools,
        [
            "follow_up",
            "inspect",
            "interrupt",
            "read_items",
            "respond",
            "spawn",
            "wait"
        ]
    );

    let claude = peer
        .tool("spawn", json!({"provider":"claude","prompt":"first"}))
        .await;
    let codex = peer
        .tool("spawn", json!({"provider":"codex","prompt":"first"}))
        .await;
    assert_ne!(claude["session"], codex["session"]);
    let first = peer
        .tool(
            "wait",
            json!({"runs":[claude["run"],codex["run"]],"timeout_ms":10000}),
        )
        .await;
    assert_eq!(first["condition_met"], true, "{first}");
    for snapshot in first["runs"].as_array().unwrap() {
        assert_eq!(snapshot["result"]["text"], "turn 1");
    }

    let mut cursors = [0, 0];
    for (index, initial) in [&claude, &codex].into_iter().enumerate() {
        let session = &initial["session"];
        let second = peer
            .tool("follow_up", json!({"session":session,"prompt":"ask"}))
            .await;
        assert_eq!(second["session"], *session);
        assert_ne!(second["run"], initial["run"]);
        let pending = peer.awaiting_input(&second["run"]).await;
        let observed = peer
            .tool("wait", json!({"runs":[second["run"]],"timeout_ms":0}))
            .await;
        assert_eq!(observed["condition_met"], false);
        let token = &pending["interactions"][0]["id"];
        let items = peer.read_all(session, &mut cursors[index]).await;
        let interaction = items
            .iter()
            .find(|item| item["kind"] == "interaction")
            .unwrap();
        assert_eq!(interaction["run"], second["run"]);
        assert_eq!(interaction["interaction"], *token);
        assert!(interaction["data"]["request"].is_object());
        assert!(
            items
                .iter()
                .any(|item| item["run"] == initial["run"] && item["kind"] == "run_finished")
        );
        assert!(
            items
                .iter()
                .any(|item| item["data"].to_string().contains("fixturePayload"))
        );

        let (reply, expected) = if index == 0 {
            (json!({"behavior":"allow"}), "permission allowed")
        } else {
            (json!({"decision":"decline"}), "permission denied")
        };
        let receipt = peer
            .tool("respond", json!({"interaction":token,"response":reply}))
            .await;
        assert_eq!(receipt["interaction"], *token);
        let finished = peer
            .tool("wait", json!({"runs":[second["run"]],"timeout_ms":10000}))
            .await;
        assert_eq!(finished["condition_met"], true);
        assert_eq!(finished["runs"][0]["result"]["text"], expected);

        let tail = peer.read_all(session, &mut cursors[index]).await;
        assert!(
            tail.iter()
                .any(|item| item["run"] == second["run"] && item["kind"] == "run_finished")
        );
        let stale = peer
            .call(
                "tools/call",
                json!({
                    "name":"respond","arguments":{"interaction":token,"response":reply}
                }),
            )
            .await;
        assert_eq!(stale["isError"], true);
        assert_eq!(
            stale["structuredContent"]["error"]["kind"],
            "unknown_interaction"
        );
    }

    let interrupted = peer
        .tool(
            "follow_up",
            json!({"session":claude["session"],"prompt":"ask"}),
        )
        .await;
    peer.awaiting_input(&interrupted["run"]).await;
    let cancelled = peer
        .tool("interrupt", json!({"run":interrupted["run"]}))
        .await;
    assert_eq!(cancelled["result"]["status"], "cancelled", "{cancelled}");

    let held = peer
        .tool(
            "follow_up",
            json!({"session":codex["session"],"prompt":"ask"}),
        )
        .await;
    peer.awaiting_input(&held["run"]).await;
    let wait_id = peer
        .request(
            "tools/call",
            json!({
                "name":"wait","arguments":{"runs":[held["run"]],"timeout_ms":10000}
            }),
        )
        .await;
    drop(peer.input);
    let response = timeout(DEADLINE, peer.output.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], wait_id);
    assert_eq!(
        response["result"]["structuredContent"]["runs"][0]["result"]["status"], "cancelled",
        "{response}"
    );
    assert!(
        timeout(DEADLINE, peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    for provider in ["claude", "codex"] {
        let fixture = workspace.path().join(provider);
        assert!(fixture.join("exited").is_file());
        let cwd = fs::read_to_string(fixture.join("native-cwd")).unwrap();
        assert_eq!(
            Path::new(cwd.trim()),
            workspace.path().join("worker-cwd").canonicalize().unwrap()
        );
    }
}

#[tokio::test]
async fn broken_stdout_settles_a_worker_with_stdin_open() {
    let workspace = tempfile::tempdir().unwrap();
    let mut peer = Peer::start(workspace.path(), None);
    peer.initialize().await;
    let run = peer
        .tool("spawn", json!({"provider":"codex","prompt":"ask"}))
        .await;
    peer.awaiting_input(&run["run"]).await;

    drop(peer.output);
    let ping = json!({"jsonrpc":"2.0","id":peer.next_id,"method":"ping"});
    timeout(
        DEADLINE,
        peer.input.write_all(format!("{ping}\n").as_bytes()),
    )
    .await
    .unwrap()
    .unwrap();
    let status = timeout(DEADLINE, peer.child.wait())
        .await
        .expect("server did not stop after the stdout failure")
        .unwrap();
    assert_eq!(status.code(), Some(1));
    assert!(workspace.path().join("codex/exited").is_file());
}

#[tokio::test]
async fn profile_selection_freezes_native_options_across_follow_up() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let catalog = root.join("profiles.json");
    let profiles = json!({
        "schema_version": 1,
        "profiles": [
            {"name":"worker","provider":"claude","model":"fixture-claude",
             "native_args":["--restricted","--strict-mcp-config","--tools","Read,Edit"]},
            {"name":"reviewer","provider":"codex","model":"fixture-codex",
             "native_args":["-c","sandbox_mode=\"read-only\"","-c","approval_policy=\"never\""]}
        ]
    });
    fs::write(&catalog, profiles.to_string()).unwrap();
    let mut peer = Peer::start(root, Some(&catalog));
    peer.initialize().await;
    let listed = peer.call("tools/list", json!({})).await;
    let spawn = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "spawn")
        .unwrap();
    let schema = &spawn["inputSchema"];
    assert_eq!(schema["required"], json!(["prompt", "profile"]));
    assert_eq!(
        schema["properties"]["profile"]["enum"],
        json!(["reviewer", "worker"])
    );
    assert!(schema["properties"].get("provider").is_none());

    // Invalid selectors cannot spawn a process, even when a provider is valid.
    for arguments in [
        json!({"profile":"../worker","prompt":"first"}),
        json!({"provider":"claude","prompt":"first"}),
        json!({"provider":"claude","profile":"worker","prompt":"first"}),
        json!({"prompt":"first"}),
    ] {
        let result = peer
            .call("tools/call", json!({"name":"spawn","arguments":arguments}))
            .await;
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["error"]["kind"],
            "invalid_input"
        );
    }
    for provider in ["claude", "codex"] {
        assert!(!root.join(provider).join("argv").exists());
    }
    // The catalog belongs to the server for this connection and is loaded once.
    fs::write(&catalog, "changed after initialization").unwrap();
    for (profile, provider, arguments) in [
        (
            "worker",
            "claude",
            &[
                "--restricted",
                "--strict-mcp-config",
                "--tools",
                "Read,Edit",
            ][..],
        ),
        (
            "reviewer",
            "codex",
            &[
                "-c",
                "sandbox_mode=\"read-only\"",
                "-c",
                "approval_policy=\"never\"",
            ][..],
        ),
    ] {
        let started = peer
            .tool("spawn", json!({"profile":profile,"prompt":"first"}))
            .await;
        assert_eq!(started["provider"], provider);
        let first = peer
            .tool("wait", json!({"runs":[started["run"]],"timeout_ms":10000}))
            .await;
        assert_eq!(first["runs"][0]["result"]["text"], "turn 1");
        assert_eq!(
            first["runs"][0]["saved_session"]["worker"]["native_args"],
            json!(arguments)
        );
        let argv_path = root.join(provider).join("argv");
        let argv = fs::read_to_string(&argv_path).unwrap();
        let tokens: Vec<_> = argv.split_terminator('\0').collect();
        assert!(
            tokens
                .windows(arguments.len())
                .any(|window| window == arguments),
            "{tokens:?}"
        );

        let followed = peer
            .tool(
                "follow_up",
                json!({"session":started["session"],"prompt":"follow up"}),
            )
            .await;
        let second = peer
            .tool("wait", json!({"runs":[followed["run"]],"timeout_ms":10000}))
            .await;
        assert_ne!(followed["run"], started["run"]);
        assert_eq!(followed["session"], started["session"]);
        assert_eq!(
            second["runs"][0]["result"]["session_id"],
            first["runs"][0]["result"]["session_id"]
        );
        assert_eq!(
            second["runs"][0]["saved_session"]["worker"]["native_args"],
            json!(arguments)
        );
        assert_eq!(fs::read_to_string(argv_path).unwrap(), argv);
    }
    drop(peer.input);
    assert!(
        timeout(DEADLINE, peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn invalid_catalog_is_rejected_before_mcp_startup() {
    let root = tempfile::tempdir().unwrap();
    let catalog = root.path().join("profiles.json");
    let worker = json!({"name":"worker","provider":"claude"});
    for (profiles, expected) in [
        (json!([]), "at least one profile"),
        (json!([worker, worker]), "duplicate profile"),
        (
            json!([{"name":"../worker","provider":"claude"}]),
            "profile names",
        ),
        (
            json!([{"name":"worker","provider":"claude","binary":"/bin/sh"}]),
            "unknown field",
        ),
        (
            json!([{"name":"worker","provider":"claude","native_args":["--resume","other"]}]),
            "unsupported by resident",
        ),
        (
            json!([{"name":"worker","provider":"claude","native_args":["--restricted=false"]}]),
            "does not take a value",
        ),
        (
            json!([{"name":"reviewer","provider":"codex","native_args":["-c","sandbox_mode=\"invalid\""]}]),
            "invalid value",
        ),
    ] {
        fs::write(
            &catalog,
            json!({"schema_version":1,"profiles":profiles}).to_string(),
        )
        .unwrap();
        let output = timeout(
            DEADLINE,
            Command::new(env!("CARGO_BIN_EXE_due"))
                .arg("mcp")
                .arg("--profiles")
                .arg(&catalog)
                .current_dir(root.path())
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(expected), "{stderr}");
    }
}
