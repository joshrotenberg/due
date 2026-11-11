use super::*;

#[test]
fn claude_denies_excess_permission_without_closing_session() {
    let mut client = Client::new(false);
    let session = client.open("claude", true, None);
    let turn = client.start(&session, "permission-overflow");
    let excess = client.pushed_until(|item| {
        item["kind"] == "native_request" && item["data"]["request_id"] == "permission-33"
    });
    assert!(excess["turn_id"].is_null(), "{excess}");
    let read = client.ok("turn.read", json!({"session_id":session,"turn_id":turn}));
    assert_eq!(read["state"], "awaiting_input", "{read}");
    let pending = read["interactions"].as_array().unwrap();
    assert_eq!(pending.len(), 32, "{read}");
    for number in 1..=32 {
        assert!(
            pending
                .iter()
                .any(|item| item["request_id"] == format!("permission-{number}")),
            "missing permission-{number}: {read}"
        );
    }

    client.ok(
        "turn.interrupt",
        json!({"session_id":session,"turn_id":turn}),
    );
    assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
    let next = client.start(&session, "ordinary");
    let completed = client.finished(&session, &next);
    assert_eq!(completed["status"], "succeeded", "{completed}");
    assert_eq!(completed["text"], "turn 2");
    assert_eq!(completed["session_id"], "native-claude");
    client.close(&session);
    client.eof();

    let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
    let responses: Vec<Value> = input
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|item| item["type"] == "control_response")
        .collect();
    assert!(
        responses.iter().any(|item| {
            item["response"]["request_id"] == "permission-33"
                && item["response"]["response"]["behavior"] == "deny"
                && item["response"]["response"]["message"] == "Too many pending permission requests"
        }),
        "{responses:?}"
    );
    assert!(
        responses
            .iter()
            .all(|item| item["response"]["response"]["behavior"] != "allow"),
        "must never auto-approve: {responses:?}"
    );
}

#[test]
fn codex_rejects_excess_permission_and_fails_established_session() {
    let mut client = Client::new(false);
    let session = client.open("codex", true, None);
    let turn = client.start(&session, "permission-overflow");
    client.ready("codex", &session, &turn);
    fs::write(client.dir.path().join("release-permissions"), "release").unwrap();
    let completed = client.finished(&session, &turn);
    assert_eq!(completed["status"], "failed", "{completed}");
    assert_eq!(completed["error"]["kind"], "protocol", "{completed}");
    assert!(
        completed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("32 pending interaction limit"),
        "{completed}"
    );
    assert!(client.dir.path().join("exited").exists());
    assert_eq!(
        client.call(
            "turn.start",
            json!({"session_id":session,"prompt":"ordinary"})
        )["error"]["code"],
        -32000
    );
    let records = client.ok("session.read", json!({"session_id":session,"limit":256}));
    let items = records["items"].as_array().unwrap();
    let interactions: Vec<_> = items
        .iter()
        .filter(|item| item["kind"] == "interaction")
        .collect();
    assert_eq!(interactions.len(), 32, "{records}");
    assert!(interactions.iter().all(|item| item["turn_id"] == turn));
    assert!(
        items
            .iter()
            .any(|item| { item["kind"] == "native_request" && item["data"]["id"] == 133 })
    );
    client.close(&session);
    client.eof();

    let input = fs::read_to_string(client.dir.path().join("input.jsonl")).unwrap();
    let replies: Vec<Value> = input
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|item| item.get("method").is_none())
        .collect();
    assert!(
        replies.iter().any(|item| {
            item["id"] == 133 && item["error"]["message"] == "Too many pending Due interactions"
        }),
        "{replies:?}"
    );
    assert!(
        replies.iter().all(|item| item.get("result").is_none()),
        "must never auto-approve: {replies:?}"
    );
}

#[test]
fn claude_partial_payload_is_retained_without_push_streaming() {
    let mut client = Client::new(false);
    let session = client.open("claude", false, None);
    let turn = client.start(&session, "partial-payload");
    let read = client.read_until(&session, &turn, "finished");
    assert_eq!(read["result"]["status"], "succeeded", "{read}");
    let records = client.ok("session.read", json!({"session_id":session}));
    let partial = records["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "native" && item["data"]["event"]["type"] == "stream_event")
        .expect("stream:false must preserve native partial events for polling clients");
    assert_eq!(partial["turn_id"], turn);
    assert_eq!(
        partial["data"],
        json!({
            "provider":"claude",
            "event": {
                "type":"stream_event", "session_id":"native-claude",
                "event": {
                    "type":"content_block_delta", "index":0,
                    "delta":{"type":"text_delta","text":"partial chunk\nsecond line"}
                },
                "fixturePartial":{"unknown":[true,null,{"future":42}]}
            }
        })
    );
    client.close(&session);
    client.eof();
    assert!(client.notifications.is_empty());
}
