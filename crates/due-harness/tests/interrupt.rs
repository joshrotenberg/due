//! Interrupt/terminal ordering against a scripted JSON-RPC peer. Message order is fixed by the
//! peer script: no native timings, sleeps or live models.
#![cfg(all(unix, feature = "js"))]

use due_harness::{Options, Outcome, run_js};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
use tempfile::TempDir;

const PRELUDE: &str = "const rejection=async call=>{try{await call();return null;}catch(e){return String(e.message);}};";

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let peer = dir.path().join("peer");
        fs::write(&peer, include_str!("fixtures/interrupt-peer.py")).unwrap();
        fs::set_permissions(&peer, fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    fn run(&self, body: &str) -> Outcome {
        run_js(Options {
            due: self.dir.path().join("peer"),
            cwd: self.dir.path().to_owned(),
            evidence: self.dir.path().join("evidence"),
            source: format!(
                "{PRELUDE} Due.run(async scope => {{ const worker = await scope.open({{}}); {body} }});"
            ),
            args: "{}".into(),
            timeout: Duration::from_secs(10),
        })
        .unwrap()
    }

    /// Turn ids of every turn.interrupt request the peer received, in order.
    fn interrupts(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|request| request["method"] == "turn.interrupt")
            .map(|request| request["params"]["turn_id"].as_str().unwrap().to_owned())
            .collect()
    }
}

fn finished(id: &str) -> Value {
    json!({"interrupt_requested": false, "turn_id": id, "already_finished": true})
}

/// Starts and interrupts a normal turn to show the session remains usable.
const REUSE: &str = "const next=await worker.start('other'); const nack=await worker.interrupt(next); const nstatus=(await worker.wait(next)).status;";

#[test]
fn terminal_already_present_acks_locally_and_rejects_wrong_or_consumed_ids() {
    let fixture = Fixture::new();
    let outcome = fixture.run(&format!(
        "const id=await worker.start('quick');
         const ack=await worker.interrupt(id), again=await worker.interrupt(id);
         const wrong=await rejection(()=>worker.interrupt('wrong-id'));
         const empty=await rejection(()=>worker.interrupt(''));
         const status=(await worker.wait(id)).status;
         const consumed=await rejection(()=>worker.interrupt(id));
         {REUSE}
         return {{id,ack,again,wrong,empty,status,consumed,next,nack,nstatus}};"
    ));
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    let value = &outcome.result;
    assert_eq!(value["ack"], finished("turn-1"));
    assert_eq!(value["again"], finished("turn-1"));
    for field in ["wrong", "empty", "consumed"] {
        assert_eq!(value[field], "no active turn with that id", "{field}");
    }
    assert_eq!(value["status"], "succeeded");
    // Reuse after the consumed result: a real interruption of the next turn goes out.
    assert_eq!(
        value["nack"],
        json!({"interrupt_requested": true, "turn_id": "turn-2"})
    );
    assert_eq!(value["nstatus"], "cancelled");
    assert_eq!(
        fixture.interrupts(),
        ["turn-2"],
        "only the live turn was interrupted upstream"
    );
}

#[test]
fn terminal_before_finished_rejection_resolves_as_already_finished() {
    for prompt in ["race", "race-native", "race-idle"] {
        let fixture = Fixture::new();
        let outcome = fixture.run(&format!(
            "const id=await worker.start('{prompt}');
         const ack=await worker.interrupt(id);
         const status=(await worker.wait(id)).status;
         {REUSE}
         return {{ack,status,nack,nstatus}};"
        ));
        assert!(outcome.error.is_none(), "{:?}", outcome.error);
        assert_eq!(outcome.result["ack"], finished("turn-1"));
        assert_eq!(
            outcome.result["status"], "succeeded",
            "wait still owns the terminal result"
        );
        assert_eq!(
            outcome.result["nack"],
            json!({"interrupt_requested": true, "turn_id": "turn-2"})
        );
        assert_eq!(outcome.result["nstatus"], "cancelled");
        // The interruption was sent before the terminal item arrived.
        assert_eq!(fixture.interrupts(), ["turn-1", "turn-2"]);
    }
}

#[test]
fn genuine_rejection_after_terminal_remains_an_error() {
    let fixture = Fixture::new();
    let outcome = fixture.run(&format!(
        "const id=await worker.start('genuine');
         const error=await rejection(()=>worker.interrupt(id));
         const status=(await worker.wait(id)).status;
         {REUSE}
         return {{error,status,nack,nstatus}};"
    ));
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    assert_eq!(
        outcome.result["error"],
        "RPC error -32000: interrupt delivery failed"
    );
    assert_eq!(outcome.result["status"], "succeeded");
    assert_eq!(
        outcome.result["nack"],
        json!({"interrupt_requested": true, "turn_id": "turn-2"})
    );
    assert_eq!(fixture.interrupts(), ["turn-1", "turn-2"]);
}

#[test]
fn finished_rejection_without_terminal_is_not_recovered_and_earlier_races_do_not_leak() {
    let fixture = Fixture::new();
    // The first turn leaves a recovered race behind; the second must not inherit it.
    let outcome = fixture.run(
        "const raced=await worker.start('race');
         const first=await worker.interrupt(raced);
         await worker.wait(raced);
         const early=await worker.start('early');
         const error=await rejection(()=>worker.interrupt(early));
         const retry=await worker.interrupt(early);
         const status=(await worker.wait(early)).status;
         return {first,error,retry,status};",
    );
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    assert_eq!(outcome.result["first"], finished("turn-1"));
    assert_eq!(
        outcome.result["error"],
        "RPC error -32000: turn is already finished"
    );
    assert_eq!(
        outcome.result["retry"],
        json!({"interrupt_requested": true, "turn_id": "turn-2"})
    );
    assert_eq!(outcome.result["status"], "cancelled");
    assert_eq!(fixture.interrupts(), ["turn-1", "turn-2", "turn-2"]);
}

#[test]
fn session_failure_is_not_masked_by_finished_rejection() {
    let fixture = Fixture::new();
    let outcome = fixture.run(
        "const id=await worker.start('failed');
         const ack=await worker.interrupt(id);
         return {ack};",
    );
    let error = outcome
        .error
        .expect("session failure must fail the workflow");
    assert!(error.contains("session failed"), "{error}");
    assert!(outcome.result.is_null(), "{}", outcome.result);
}
