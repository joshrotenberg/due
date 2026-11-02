use super::*;

fn options(client: &Client, request: &std::path::Path) -> Value {
    json!({"request":request,"binary":client.dir.path().join("provider"),"stream":true})
}

fn profile(client: &Client, value: Value) -> std::path::PathBuf {
    let path = client.dir.path().join("profile.json");
    fs::write(&path, value.to_string()).unwrap();
    path
}

fn argv(client: &Client) -> Vec<String> {
    fs::read(client.dir.path().join("argv"))
        .unwrap()
        .strip_suffix(&[0])
        .unwrap()
        .split(|byte| *byte == 0)
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .collect()
}

fn contains_args(actual: &[String], expected: &[&str]) -> bool {
    actual.windows(expected.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
    })
}

#[test]
fn profiles_resolve_once_and_preserve_native_policy_for_multiple_turns() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let native_args = if provider == "claude" {
            json!([
                "--allowedTools",
                "Read,Grep",
                "--disallowed-tools=Agent",
                "--permission-mode",
                "plan"
            ])
        } else {
            json!([
                "-c",
                "sandbox_mode=\"read-only\"",
                "--config=approval_policy=\"never\"",
                "-cagents.enabled=false"
            ])
        };
        let request = profile(
            &client,
            json!({
                "schema_version":1,"provider":provider,"cwd":client.dir.path(),
                "model":"fixture-model","effort":"high","timeout":"20s",
                "ephemeral":true,"native_args":native_args,
            }),
        );
        let mut input = options(&client, &request);
        input["ephemeral"] = json!(false);
        let opened = client.ok("session.open", input);
        let effective = &opened["resolved_options"];
        assert_eq!(effective["native_args"], native_args);
        assert_eq!(effective["provider"], provider);
        assert_eq!(effective["model"], "fixture-model");
        assert_eq!(effective["effort"], "high");
        assert_eq!(effective["timeout"], "20s");
        assert_eq!(effective["ephemeral"], false);
        assert_eq!(effective["stream"], true);
        assert_eq!(
            effective["cwd"],
            client.dir.path().canonicalize().unwrap().to_str().unwrap()
        );
        let session = opened["session_id"].as_str().unwrap();
        let opened_item = client
            .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_opened");
        assert_eq!(opened_item["data"]["resolved_options"], *effective);
        // Later edits cannot change the already opened worker's configuration.
        fs::write(&request, "not JSON anymore").unwrap();
        for prompt in ["first", "second"] {
            let turn = client.start(session, prompt);
            assert_eq!(client.finished(session, &turn)["status"], "succeeded");
        }
        let actual = argv(&client);
        let expected: Vec<_> = native_args
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert!(contains_args(&actual, &expected), "{actual:?}");
        if provider == "claude" {
            assert!(contains_args(
                &actual,
                &["--permission-prompt-tool", "stdio"]
            ));
            assert!(contains_args(&actual, &["--model", "fixture-model"]));
        } else {
            assert!(contains_args(
                &actual,
                &["app-server", "--listen", "stdio://"]
            ));
            let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
            let messages: Vec<Value> = input
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(
                messages
                    .iter()
                    .any(|message| message["method"] == "thread/start"
                        && message["params"]["model"] == "fixture-model")
            );
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message["method"] == "turn/start"
                        && message["params"]["effort"] == "high")
                    .count(),
                2
            );
        }
        client.close(session);
        client.eof();
    }
}

#[test]
fn checked_in_workspace_profile_keeps_every_native_restriction() {
    let mut client = Client::new(false);
    let value: Value =
        serde_json::from_str(include_str!("../../examples/codex-workspace-profile.json")).unwrap();
    let request = profile(&client, value.clone());
    let mut input = options(&client, &request);
    input["cwd"] = json!(client.dir.path());
    let opened = client.ok("session.open", input);
    assert_eq!(
        opened["resolved_options"]["native_args"],
        value["native_args"]
    );
    let expected: Vec<_> = value["native_args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(contains_args(&argv(&client), &expected));
    let session = opened["session_id"].as_str().unwrap();
    client.close(session);
    client.eof();
}

#[test]
fn explicit_null_false_and_replacement_values_override_profile_defaults() {
    let mut client = Client::new(false);
    let request = profile(
        &client,
        json!({
            "schema_version":1,"provider":"replaced-invalid-provider","model":"",
            "effort":"invalid","timeout":"invalid","cwd":"missing-directory",
            "resume":"existing","ephemeral":true,"native_args":["--unsupported"],
        }),
    );
    let mut input = options(&client, &request);
    input.as_object_mut().unwrap().extend(
        json!({
            "provider":"codex","model":null,"effort":null,"timeout":null,
            "cwd":null,"resume":null,"ephemeral":false,"native_args":[],
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let opened = client.ok("session.open", input);
    let effective = &opened["resolved_options"];
    for key in ["model", "effort", "timeout", "resume"] {
        assert!(effective[key].is_null(), "{key}: {effective}");
    }
    assert_eq!(
        effective["cwd"],
        std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(effective["native_args"], json!([]));
    assert_eq!(effective["ephemeral"], false);
    client.close(opened["session_id"].as_str().unwrap());
    client.eof();
}

#[test]
fn profile_structure_and_task_fields_are_rejected_before_native_launch() {
    for source in [
        r#"{"schema_version":1,"provider":null}"#,
        r#"{"schema_version":1,"timeout":3}"#,
        r#"{"schema_version":1,"model":"first","model":"second"}"#,
        r#"{"schema_version":1,"unknown":true}"#,
        r#"{"schema_version":2}"#,
        r#"{"schema_version":1,"native_args":null}"#,
        r#"{"schema_version":1,"prompt":"not an automatic first turn"}"#,
        r#"{"schema_version":1,"schema":"answer.json"}"#,
        r#"{"schema_version":1} {}"#,
    ] {
        let mut client = Client::new(false);
        let request = client.dir.path().join("profile.json");
        fs::write(&request, source).unwrap();
        let mut input = options(&client, &request);
        input.as_object_mut().unwrap().extend(
            json!({"provider":"codex","model":"replacement","timeout":null,"native_args":[]})
                .as_object()
                .unwrap()
                .clone(),
        );
        let reply = client.call("session.open", input);
        assert_eq!(reply["error"]["code"], -32602, "{source}: {reply}");
        assert!(!client.dir.path().join("pid").exists());
        let next = client.open("codex", false, None);
        client.close(&next);
        client.eof();
    }
}

#[test]
fn unsupported_policy_and_provider_switch_fail_without_dropping_options() {
    for (provider, arguments) in [
        ("claude", json!(["--permission-prompt-tool", "other"])),
        ("claude", json!(["--output-format", "json"])),
        (
            "claude",
            json!(["--allowedTools", "Read", "--resume", "other"]),
        ),
        ("codex", json!(["--json"])),
        ("codex", json!(["-c", "model=\"hidden\""])),
        (
            "codex",
            json!(["-c", "sandbox_mode=\"workspace-write\"\nmodel=\"hidden\""]),
        ),
        (
            "codex",
            json!(["-c", "sandbox_workspace_write.network_access=\"false\""]),
        ),
        ("codex", json!(["--config", "cwd=\"other\""])),
        ("codex", json!(["-c", "[agents]\nenabled=false"])),
        ("codex", json!(["-c", "agents={enabled=false}"])),
    ] {
        let mut client = Client::new(false);
        let request = profile(
            &client,
            json!({"schema_version":1,"provider":provider,"native_args":arguments}),
        );
        let reply = client.call("session.open", options(&client, &request));
        assert_eq!(reply["error"]["code"], -32000, "{reply}");
        assert!(!client.dir.path().join("pid").exists());
        client.eof();
    }
    let mut client = Client::new(false);
    let request = profile(
        &client,
        json!({"schema_version":1,"provider":"codex","native_args":["-c","agents.enabled=false"]}),
    );
    let mut input = options(&client, &request);
    input["provider"] = json!("claude");
    let reply = client.call("session.open", input);
    assert_eq!(reply["error"]["code"], -32000);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unsupported")
    );
    assert!(!client.dir.path().join("pid").exists());
    client.eof();
}

#[test]
fn profile_paths_are_relative_to_server_cwd_not_the_profile_directory() {
    let mut client = Client::new(false);
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let directory = tempfile::tempdir_in(&cwd).unwrap();
    let request = directory.path().join("profile.json");
    fs::write(
        &request,
        r#"{"schema_version":1,"provider":"codex","cwd":"tests/fixtures"}"#,
    )
    .unwrap();
    let input = options(&client, request.strip_prefix(&cwd).unwrap());
    let opened = client.ok("session.open", input);
    let expected = cwd.join("tests/fixtures").canonicalize().unwrap();
    assert_eq!(
        opened["resolved_options"]["cwd"],
        expected.to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(client.dir.path().join("native-cwd"))
            .unwrap()
            .trim(),
        expected.to_str().unwrap()
    );
    client.close(opened["session_id"].as_str().unwrap());
    client.eof();
}

#[test]
fn invalid_profile_sources_are_bounded_and_leave_connection_usable() {
    let mut client = Client::new(false);
    let oversized = client.dir.path().join("large.json");
    fs::write(&oversized, " ".repeat(1024 * 1024 + 1)).unwrap();
    let fifo = client.dir.path().join("profile.fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    for request in [
        std::path::PathBuf::from("-"),
        client.dir.path().join("absent"),
        client.dir.path().to_owned(),
        oversized,
        fifo,
    ] {
        let reply = client.call("session.open", options(&client, &request));
        assert_eq!(reply["error"]["code"], -32602, "{request:?}: {reply}");
        assert!(!client.dir.path().join("pid").exists());
    }
    let next = client.open("claude", false, None);
    client.close(&next);
    client.eof();
}

#[test]
fn rpc_required_types_and_merged_resume_conflict_are_preserved() {
    let mut client = Client::new(false);
    for key in ["provider", "native_args", "ephemeral", "stream", "request"] {
        let mut input = json!({"provider":"codex"});
        input[key] = Value::Null;
        assert_eq!(
            client.call("session.open", input)["error"]["code"],
            -32602,
            "{key}"
        );
    }
    assert_eq!(
        client.call("session.open", json!({}))["error"]["code"],
        -32602
    );
    let request = profile(
        &client,
        json!({"schema_version":1,"provider":"codex","resume":"existing","ephemeral":true}),
    );
    let reply = client.call("session.open", options(&client, &request));
    assert_eq!(reply["error"]["code"], -32000);
    assert!(!client.dir.path().join("pid").exists());
    let mut input = options(&client, &request);
    input["ephemeral"] = json!(false);
    let opened = client.ok("session.open", input);
    assert_eq!(opened["resolved_options"]["resume"], "existing");
    client.close(opened["session_id"].as_str().unwrap());
    client.eof();
}
