use super::*;

fn code(response: &Value) -> Option<i64> {
    response["error"]["code"].as_i64()
}

/// Calls `method` and records a failure message unless the response matches
/// `expected` (`Some(code)` for an error code, `None` for success).
fn expect(
    client: &mut Client,
    failures: &mut Vec<String>,
    method: &str,
    params: Value,
    expected: Option<i64>,
) {
    let response = client.call(method, params.clone());
    if code(&response) != expected || (expected.is_none() && response.get("result").is_none()) {
        failures.push(format!(
            "{method} {params}: expected {expected:?}, got {response}"
        ));
    }
}

#[test]
fn rpc_numeric_ranges_are_invalid_params_and_connection_stays_usable() {
    let mut client = Client::new(false);
    let session = client.open("claude", false, None);
    let turn = client.start(&session, "ordinary");
    let finished = client.read_until(&session, &turn, "finished");
    assert_eq!(finished["result"]["status"], "succeeded", "{finished}");

    // Failures are collected so the session is always closed and the fixture
    // process settled before the test reports them.
    let mut failures = Vec::new();
    let max = u64::MAX;

    // session.read limit: valid controls.
    for limit in [json!(1), json!(256)] {
        expect(
            &mut client,
            &mut failures,
            "session.read",
            json!({"session_id":session,"limit":limit}),
            None,
        );
    }
    expect(
        &mut client,
        &mut failures,
        "session.read",
        json!({"session_id":session}),
        None,
    );

    // session.read limit: out of range or wrong type.
    for limit in [
        json!(0),
        json!(257),
        json!(max),
        json!(-1),
        json!(1.5),
        json!("2"),
        json!(true),
        json!(null),
        json!([]),
        json!({}),
    ] {
        expect(
            &mut client,
            &mut failures,
            "session.read",
            json!({"session_id":session,"limit":limit}),
            Some(-32602),
        );
    }

    // turn.read wait_ms: valid controls.
    for wait in [json!(0), json!(10000)] {
        expect(
            &mut client,
            &mut failures,
            "turn.read",
            json!({"session_id":session,"turn_id":turn,"wait_ms":wait}),
            None,
        );
    }
    expect(
        &mut client,
        &mut failures,
        "turn.read",
        json!({"session_id":session,"turn_id":turn}),
        None,
    );

    // turn.read wait_ms: out of range or wrong type.
    for wait in [
        json!(10001),
        json!(max),
        json!(-1),
        json!(1.5),
        json!("5"),
        json!(true),
        json!(null),
        json!([]),
        json!({}),
    ] {
        expect(
            &mut client,
            &mut failures,
            "turn.read",
            json!({"session_id":session,"turn_id":turn,"wait_ms":wait}),
            Some(-32602),
        );
    }

    // Unknown sessions: valid numbers reach the manager (-32000); invalid
    // numbers are rejected as parameters first (-32602).
    expect(
        &mut client,
        &mut failures,
        "session.read",
        json!({"session_id":"missing","limit":1}),
        Some(-32000),
    );
    expect(
        &mut client,
        &mut failures,
        "session.read",
        json!({"session_id":"missing","limit":257}),
        Some(-32602),
    );
    expect(
        &mut client,
        &mut failures,
        "turn.read",
        json!({"session_id":"missing","turn_id":"missing","wait_ms":0}),
        Some(-32000),
    );
    expect(
        &mut client,
        &mut failures,
        "turn.read",
        json!({"session_id":"missing","turn_id":"missing","wait_ms":10001}),
        Some(-32602),
    );

    expect(
        &mut client,
        &mut failures,
        "turn.read",
        json!({"session_id":session,"turn_id":"missing","wait_ms":0}),
        Some(-32000),
    );
    expect(
        &mut client,
        &mut failures,
        "turn.read",
        json!({"session_id":session,"turn_id":"missing","wait_ms":10001}),
        Some(-32602),
    );

    // The connection and session remain usable after all the invalid input.
    let next = client.start(&session, "ordinary");
    let completed = client.read_until(&session, &next, "finished");
    if completed["result"]["status"] != "succeeded" {
        failures.push(format!("follow-up turn did not succeed: {completed}"));
    }

    client.close(&session);
    client.eof();
    assert!(
        failures.is_empty(),
        "numeric range failures:\n{}",
        failures.join("\n")
    );
}
