use super::*;

#[test]
fn closing_a_failed_session_reports_cleanup_separately_for_both_providers() {
    for provider in ["claude", "codex"] {
        for (prompt, timeout) in [("hold", Some("200ms")), ("native-exit", None)] {
            let mut client = Client::new(false);
            let session = client.open(provider, true, timeout);
            let turn = client.start(&session, prompt);
            let result = client.finished(&session, &turn);
            assert_eq!(
                result["status"],
                if timeout.is_some() {
                    "timed_out"
                } else {
                    "failed"
                },
                "{provider} {prompt}: {result}"
            );
            let failed = client.pushed_until(|item| {
                item["session_id"] == session && item["kind"] == "session_failed"
            });
            let closed = client.ok("session.close", json!({"session_id":session}));
            assert_eq!(closed["closed"], true);
            assert_eq!(closed["native_session_id"], format!("native-{provider}"));
            assert_eq!(closed["session_error"], failed["data"]["message"]);
            let item = client.pushed_until(|item| {
                item["session_id"] == session && item["kind"] == "session_closed"
            });
            assert!(item["data"]["error"].is_null(), "{item}");
            assert_eq!(item["data"]["session_error"], closed["session_error"]);
            assert_eq!(
                item["data"]["native_session_id"],
                closed["native_session_id"]
            );
            assert!(client.dir.path().join("exited").exists());
            assert_eq!(
                client.call("session.read", json!({"session_id":session}))["error"]["code"],
                -32000
            );
            let replacement = client.open(provider, false, None);
            client.close(&replacement);
            client.eof();
        }
    }
}

#[test]
fn explicit_close_cancels_without_a_historical_session_error() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        let closed = client.ok("session.close", json!({"session_id":session}));
        assert!(closed["session_error"].is_null(), "{closed}");
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        let item = client
            .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_closed");
        assert!(item["data"]["error"].is_null(), "{item}");
        assert!(item["data"]["session_error"].is_null(), "{item}");
        assert_eq!(
            item["data"]["native_session_id"],
            format!("native-{provider}")
        );
        for method in ["session.read", "turn.read"] {
            let mut params = json!({"session_id":session});
            if method == "turn.read" {
                params["turn_id"] = json!(turn);
            }
            assert_eq!(
                client.call(method, params)["error"]["code"],
                -32000,
                "{method} must reject a removed read view"
            );
        }
        client.eof();
    }
}

#[test]
fn actual_backend_cleanup_error_still_fails_close_and_keeps_identifiers() {
    // The pinned Codex wrapper reports nonzero child exits from shutdown().
    let mut client = Client::new(false);
    let session = client.open("codex", true, None);
    let turn = client.start(&session, "native-exit-error");
    assert_eq!(client.finished(&session, &turn)["status"], "failed");
    let failed = client
        .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_failed");
    let response = client.call("session.close", json!({"session_id":session}));
    assert_eq!(response["error"]["code"], -32000, "{response}");
    let closed = client
        .pushed_until(|item| item["session_id"] == session && item["kind"] == "session_closed");
    assert!(closed["data"]["error"].is_string(), "{closed}");
    assert_eq!(closed["data"]["error"], response["error"]["message"]);
    assert_eq!(closed["data"]["session_error"], failed["data"]["message"]);
    assert_eq!(closed["data"]["native_session_id"], "native-codex");
    assert_eq!(
        client.call("session.read", json!({"session_id":session}))["error"]["code"],
        -32000
    );
    let replacement = client.open("codex", false, None);
    client.close(&replacement);
    client.eof();
}

#[test]
fn failed_open_streams_provisional_evidence_without_publishing_a_handle() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let response = client.call(
            "session.open",
            json!({
                "provider":provider,
                "stream":true,
                "binary":client.dir.path().join("nonexistent-provider"),
                "cwd":client.dir.path(),
            }),
        );
        assert_eq!(response["error"]["code"], -32000, "{response}");
        let failed = client.pushed_until(|item| item["kind"] == "session_failed");
        let provisional = failed["session_id"].as_str().unwrap();
        assert!(!provisional.is_empty());
        assert_eq!(failed["data"]["message"], response["error"]["message"]);
        for method in ["session.read", "session.close"] {
            assert_eq!(
                client.call(method, json!({"session_id":provisional}))["error"]["code"],
                -32000,
                "failed open must not publish its provisional handle"
            );
        }
        let session = client.open(provider, false, None);
        assert_ne!(session, provisional);
        client.close(&session);
        client.eof();
    }
}
