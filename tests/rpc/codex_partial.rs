use super::*;

fn partial(client: &mut Client, session: &str, turn: &str, number: usize) {
    client.pushed_until(|item| {
        item["session_id"] == session
            && item["turn_id"] == turn
            && item["kind"] == "native"
            && item["data"]["method"] == "fixture/partial-ready"
    });
    assert!(client.notifications.iter().any(|message| {
        message["params"]["turn_id"] == turn
            && message["params"]["data"]["params"]["item"]["text"]
                == format!("Partial turn {number}. é")
    }));
}

#[test]
fn codex_partial_interrupt_retains_text_and_resets_between_turns() {
    let mut client = Client::new(false);
    let session = client.open("codex", true, None);
    let mut results = Vec::new();
    for number in 1..=2 {
        let turn = client.start(&session, "codex-partial-hold");
        partial(&mut client, &session, &turn, number);
        assert_eq!(
            client.ok(
                "turn.interrupt",
                json!({"session_id":session,"turn_id":turn})
            )["interrupt_requested"],
            true
        );
        let result = client.finished(&session, &turn);
        assert_eq!(result["status"], "cancelled");
        assert_eq!(result["error"]["kind"], "cancelled");
        assert_eq!(result["session_id"], "native-codex");
        assert_eq!(result["run_id"], turn);
        assert_eq!(
            client.ok("turn.read", json!({"session_id":session,"turn_id":turn}))["result"],
            result
        );
        results.push((result, format!("Partial turn {number}. é")));
    }
    let empty = client.start(&session, "hold");
    client.ready("codex", &session, &empty);
    client.ok(
        "turn.interrupt",
        json!({"session_id":session,"turn_id":empty}),
    );
    assert_eq!(client.finished(&session, &empty)["text"], "");
    let next = client.start(&session, "ordinary");
    let next = client.finished(&session, &next);
    assert_eq!(next["status"], "succeeded");
    assert_eq!(next["text"], "turn 4");
    client.close(&session);
    client.eof();
    for (result, expected) in results {
        assert_eq!(result["text"], expected, "{result}");
    }
}

#[test]
fn codex_partial_failed_turn_keeps_text_without_promoting_successful_commentary() {
    let mut client = Client::new(false);
    let session = client.open("codex", true, None);
    let turn = client.start(&session, "codex-partial-failed");
    let failed = client.finished(&session, &turn);
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["error"]["kind"], "provider");
    let turn = client.start(&session, "codex-partial-success");
    let success = client.finished(&session, &turn);
    assert_eq!(success["status"], "succeeded");
    assert_eq!(
        success["text"], "",
        "commentary must not become a final answer"
    );
    let turn = client.start(&session, "codex-partial-final");
    let final_answer = client.finished(&session, &turn);
    assert_eq!(final_answer["status"], "succeeded");
    assert_eq!(final_answer["text"], "authoritative final answer");
    client.close(&session);
    client.eof();
    assert_eq!(failed["text"], "Partial turn 1. é", "{failed}");
}

#[test]
fn codex_partial_deadline_and_native_exit_keep_accepted_text() {
    for exit in [false, true] {
        let mut client = Client::new(false);
        let session = client.open("codex", true, (!exit).then_some("1s"));
        let turn = client.start(
            &session,
            if exit {
                "codex-partial-exit"
            } else {
                "codex-partial-hold"
            },
        );
        partial(&mut client, &session, &turn, 1);
        if exit {
            fs::write(client.dir.path().join("release-exit"), "release").unwrap();
        }
        let result = client.finished(&session, &turn);
        assert_eq!(result["status"], if exit { "failed" } else { "timed_out" });
        assert_eq!(
            result["error"]["kind"],
            if exit { "protocol" } else { "timeout" }
        );
        if exit {
            assert_eq!(
                client.call("session.close", json!({"session_id":session}))["error"]["code"],
                -32000
            );
            assert!(client.dir.path().join("exited").exists());
        } else {
            client.close(&session);
        }
        client.eof();
        assert_eq!(result["text"], "Partial turn 1. é", "{result}");
    }
}
