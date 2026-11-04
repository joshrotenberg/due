#![cfg(all(feature = "core", unix))]

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};

use due::{
    core::*,
    result::{ErrorKind as RunErrorKind, Status},
};
use tempfile::TempDir;

struct Fixture {
    directory: TempDir,
    binary: PathBuf,
    provider: Provider,
}
impl Fixture {
    fn new(provider: Provider) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = include_str!("fixtures/session-agent.sh")
            .replace("native=\"native-$provider\"", r#"native="native-$provider-$$"
previous=
for argument in "$@"; do
  if [ "$previous" = --resume ] && [ -f "$DUE_SESSION_FIXTURE_DIR/history-$argument" ]; then
    native="$argument"
    count=$(cat "$DUE_SESSION_FIXTURE_DIR/history-$native")
  fi
  previous="$argument"
done"#)
            .replace("count=$((count + 1))", "count=$((count + 1)); printf '%s' \"$count\" > \"$DUE_SESSION_FIXTURE_DIR/history-$native\"")
            .replace("        thread/start|thread/resume)", r#"        thread/start|thread/resume)
          resume_re='"threadId":"([^"]+)"'
          if [ "$method" = thread/resume ] && [[ "$line" =~ $resume_re ]]; then
            requested="${BASH_REMATCH[1]}"
            if [ -f "$DUE_SESSION_FIXTURE_DIR/history-$requested" ]; then
              native="$requested"
              count=$(cat "$DUE_SESSION_FIXTURE_DIR/history-$native")
            fi
          fi"#);
        let source = source.replace("            finish interrupted ''\n          elif", "            finish interrupted ''\n            : > \"$DUE_SESSION_FIXTURE_DIR/interrupt-seen\"\n            while [ ! -f \"$DUE_SESSION_FIXTURE_DIR/release-ack\" ]; do /bin/sleep 0.01; done\n          elif");
        fs::write(root.join("native"), source).unwrap();
        let binary = root.join("provider");
        fs::write(
            &binary,
            r#"#!/bin/sh
export DUE_SESSION_FIXTURE_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)"
if [ -f "$DUE_SESSION_FIXTURE_DIR/hold-open" ]; then
    : > "$DUE_SESSION_FIXTURE_DIR/open-waiting"
    while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-open" ]; do /bin/sleep 0.01; done
fi
exec /bin/bash "$DUE_SESSION_FIXTURE_DIR/native" "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            directory,
            binary,
            provider,
        }
    }
    fn options(&self) -> WorkerOptions {
        let mut options = WorkerOptions::new(self.provider, self.directory.path());
        options.binary = Some(self.binary.clone());
        options.model = Some("fixture-model".into());
        options.effort = Some(Effort::High);
        options
    }
    fn assignment(&self, prompt: &str) -> Assignment {
        Assignment::new(self.options(), prompt)
    }
    fn pid(&self) -> i32 {
        fs::read_to_string(self.directory.path().join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
    fn input(&self) -> String {
        fs::read_to_string(self.directory.path().join("input.jsonl")).unwrap()
    }
}
fn engine() -> Engine {
    Engine::new(Limits {
        settlement_timeout: Duration::from_secs(10),
        ..Limits::default()
    })
    .unwrap()
}
async fn finished(engine: &Engine, run: &RunRef) -> RunSnapshot {
    let waited = engine
        .wait(
            std::slice::from_ref(run),
            WaitOptions {
                until: WaitUntil::All,
                timeout: Some(Duration::from_secs(15)),
            },
        )
        .await
        .unwrap();
    assert!(waited.condition_met, "{waited:?}");
    waited.runs.into_iter().next().unwrap()
}
async fn running(engine: &Engine, run: &RunRef) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = engine.inspect(run).unwrap();
            assert_ne!(snapshot.state, RunState::Finished, "{snapshot:?}");
            if snapshot.state == RunState::Running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
async fn exists(path: PathBuf) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
async fn reaped(pid: i32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let gone = unsafe { libc::kill(pid, 0) } == -1;
            if gone {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_providers_keep_live_context_and_immutable_earlier_results() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let first = engine.spawn(fixture.assignment("first")).await.unwrap();
        let original = finished(&engine, &first).await;
        assert!(
            matches!(original.result.as_ref().unwrap().status, Status::Succeeded),
            "{original:?}"
        );
        assert_eq!(original.result.as_ref().unwrap().text, "turn 1");
        let saved = original.saved_session.as_ref().unwrap();
        assert_eq!(saved.worker.provider, provider);
        assert_eq!(
            saved.worker.cwd,
            fixture.directory.path().canonicalize().unwrap()
        );
        assert_ne!(
            serde_json::to_value(&original.run).unwrap(),
            saved.native_id
        );
        assert_ne!(
            serde_json::to_value(&original.session).unwrap(),
            saved.native_id
        );
        let before = serde_json::to_value(&original).unwrap();
        let second = engine
            .follow_up(original.session.clone(), FollowUp::new("second"))
            .await
            .unwrap();
        let revised = finished(&engine, &second).await;
        assert_eq!(revised.result.as_ref().unwrap().text, "turn 2");
        assert_eq!(revised.session, original.session);
        assert_ne!(revised.run, original.run);
        assert_eq!(
            revised.result.as_ref().unwrap().session_id,
            original.result.as_ref().unwrap().session_id
        );
        assert_eq!(
            serde_json::to_value(engine.inspect(&first).unwrap()).unwrap(),
            before
        );
        assert!(Arc::ptr_eq(
            engine.inspect(&first).unwrap().result.as_ref().unwrap(),
            original.result.as_ref().unwrap()
        ));
        let pid = fixture.pid();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "idle resident worker was lost"
        );
        engine.shutdown().await.unwrap();
        reaped(pid).await;
        engine.shutdown().await.unwrap();
        assert_eq!(
            serde_json::to_value(engine.inspect(&first).unwrap()).unwrap(),
            before
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_providers_wait_any_all_and_wait_timeout_leave_siblings_running() {
    let held = Fixture::new(Provider::Claude);
    let quick = Fixture::new(Provider::Codex);
    let engine = engine();
    let a = engine.spawn(held.assignment("hold")).await.unwrap();
    let b = engine.spawn(quick.assignment("quick")).await.unwrap();
    let refs = [a.clone(), b.clone()];
    let any = engine
        .wait(
            &refs,
            WaitOptions {
                until: WaitUntil::Any,
                timeout: Some(Duration::from_secs(10)),
            },
        )
        .await
        .unwrap();
    assert!(any.condition_met);
    assert_eq!(any.runs[1].state, RunState::Finished);
    assert_eq!(any.runs[0].run, a);
    assert_ne!(any.runs[0].state, RunState::Finished);
    running(&engine, &a).await;
    let all = engine
        .wait(
            &refs,
            WaitOptions {
                until: WaitUntil::All,
                timeout: Some(Duration::from_millis(20)),
            },
        )
        .await
        .unwrap();
    assert!(!all.condition_met);
    assert_eq!(engine.inspect(&a).unwrap().state, RunState::Running);
    assert_eq!(
        engine
            .follow_up(all.runs[0].session.clone(), FollowUp::new("busy"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Busy
    );
    let stopped = engine.interrupt(&a).await.unwrap();
    assert_eq!(stopped.state, RunState::Finished);
    assert_eq!(
        engine
            .wait(&refs, WaitOptions::default())
            .await
            .unwrap()
            .runs
            .len(),
        2
    );
    assert!(matches!(
        engine.inspect(&b).unwrap().result.as_ref().unwrap().status,
        Status::Succeeded
    ));
    let healthy = engine
        .follow_up(all.runs[1].session.clone(), FollowUp::new("still healthy"))
        .await
        .unwrap();
    assert_eq!(
        finished(&engine, &healthy)
            .await
            .result
            .as_ref()
            .unwrap()
            .text,
        "turn 2"
    );
    engine.shutdown().await.unwrap();
    reaped(held.pid()).await;
    reaped(quick.pid()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_wait_does_not_cancel_work_and_stale_interrupt_cannot_hit_the_next_turn() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = Arc::new(engine());
        let first = engine.spawn(fixture.assignment("first")).await.unwrap();
        let complete = finished(&engine, &first).await;
        let second = engine
            .follow_up(complete.session, FollowUp::new("hold"))
            .await
            .unwrap();
        running(&engine, &second).await;
        let waiter_engine = engine.clone();
        let waiter_run = second.clone();
        let waiter = tokio::spawn(async move {
            waiter_engine
                .wait(&[waiter_run], WaitOptions::default())
                .await
        });
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;
        engine.interrupt(&first).await.unwrap();
        assert_eq!(engine.inspect(&second).unwrap().state, RunState::Running);
        assert!(
            !fixture.input().contains("interrupt"),
            "stale interrupt reached native transport"
        );
        let (one, two) = tokio::join!(engine.interrupt(&second), engine.interrupt(&second));
        assert_eq!(one.unwrap().state, RunState::Finished);
        assert_eq!(two.unwrap().state, RunState::Finished);
        let third = engine
            .follow_up(
                engine.inspect(&second).unwrap().session,
                FollowUp::new("after interruption"),
            )
            .await
            .unwrap();
        assert_eq!(
            finished(&engine, &third)
                .await
                .result
                .as_ref()
                .unwrap()
                .text,
            "turn 3"
        );
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_native_resume_preserves_provider_workspace_configuration_and_history() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let original = engine();
        let run = original.spawn(fixture.assignment("first")).await.unwrap();
        let saved = finished(&original, &run).await.saved_session.unwrap();
        original.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
        let fresh = engine();
        assert_eq!(fresh.inspect(&run).unwrap_err().kind, ErrorKind::UnknownRun);
        let resumed = fresh
            .follow_up(saved.clone(), FollowUp::new("continued"))
            .await
            .unwrap();
        let snapshot = finished(&fresh, &resumed).await;
        assert_eq!(snapshot.result.as_ref().unwrap().text, "turn 2");
        assert_eq!(snapshot.saved_session.unwrap().native_id, saved.native_id);
        let evidence = if provider == Provider::Claude {
            fs::read(fixture.directory.path().join("argv")).unwrap()
        } else {
            fixture.input().into_bytes()
        };
        let evidence = String::from_utf8_lossy(&evidence);
        assert!(evidence.contains("resume"), "{evidence}");
        assert!(evidence.contains(&saved.native_id), "{evidence}");
        fresh.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execution_deadline_is_not_a_wait_deadline_and_other_workers_survive() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let first = engine.spawn(fixture.assignment("first")).await.unwrap();
        let initial = finished(&engine, &first).await;
        let run = engine
            .follow_up(
                initial.session,
                FollowUp {
                    prompt: "hold".into(),
                    timeout: Some(Duration::from_millis(100)),
                },
            )
            .await
            .unwrap();
        let result = finished(&engine, &run).await;
        assert!(
            matches!(result.result.as_ref().unwrap().status, Status::TimedOut),
            "{result:?}"
        );
        let next = engine
            .follow_up(result.session, FollowUp::new("after timeout"))
            .await
            .unwrap();
        assert!(matches!(
            finished(&engine, &next)
                .await
                .result
                .as_ref()
                .unwrap()
                .status,
            Status::Succeeded
        ));
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_interactions_fail_explicitly_without_approval_or_harming_siblings() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let good = Fixture::new(provider);
        let engine = engine();
        let held = engine.spawn(good.assignment("hold")).await.unwrap();
        let ask = engine.spawn(fixture.assignment("ask")).await.unwrap();
        let result = finished(&engine, &ask).await;
        assert_eq!(
            result.result.as_ref().unwrap().error.as_ref().unwrap().kind,
            RunErrorKind::Unsupported,
            "{result:?}"
        );
        assert_ne!(engine.inspect(&held).unwrap().state, RunState::Finished);
        assert!(!fixture.input().contains("\"behavior\":\"allow\""));
        assert_eq!(
            engine
                .follow_up(result.session, FollowUp::new("no"))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::UnknownSession
        );
        reaped(fixture.pid()).await;
        engine.shutdown().await.unwrap();
        reaped(good.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_capacity_and_foreign_handles_are_rejected_without_launching() {
    let fixture = Fixture::new(Provider::Claude);
    let engine = Engine::new(Limits {
        max_runs: 1,
        max_sessions: 1,
        ..Limits::default()
    })
    .unwrap();
    let other = self::engine();
    let mut bad = fixture.assignment("valid");
    bad.worker.native_args = vec!["--resume".into(), "smuggled".into()];
    assert_eq!(
        engine.spawn(bad).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        engine
            .spawn(fixture.assignment(" "))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    let mut bad = fixture.assignment("valid");
    bad.timeout = Some(Duration::ZERO);
    assert_eq!(
        engine.spawn(bad).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert!(!fixture.directory.path().join("pid").exists());
    let run = engine.spawn(fixture.assignment("first")).await.unwrap();
    let snapshot = finished(&engine, &run).await;
    assert_eq!(
        engine
            .spawn(fixture.assignment("extra"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Capacity
    );
    assert_eq!(other.inspect(&run).unwrap_err().kind, ErrorKind::UnknownRun);
    assert_eq!(
        other
            .follow_up(snapshot.session, FollowUp::new("foreign"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::UnknownSession
    );
    assert_eq!(
        engine
            .wait(&[], WaitOptions::default())
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        engine
            .wait(&[run.clone(), run], WaitOptions::default())
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    engine.shutdown().await.unwrap();
    assert_eq!(
        engine
            .spawn(fixture.assignment("closed"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Closed
    );
    other.shutdown().await.unwrap();
    reaped(fixture.pid()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ephemeral_live_context_does_not_offer_saved_resume_and_launch_failure_is_data() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let mut assignment = fixture.assignment("first");
        assignment.worker.ephemeral = true;
        let run = engine.spawn(assignment).await.unwrap();
        let snapshot = finished(&engine, &run).await;
        assert!(snapshot.saved_session.is_none());
        let follow = engine
            .follow_up(snapshot.session, FollowUp::new("second"))
            .await
            .unwrap();
        assert_eq!(
            finished(&engine, &follow)
                .await
                .result
                .as_ref()
                .unwrap()
                .text,
            "turn 2"
        );
        let mut missing = fixture.assignment("missing");
        missing.worker.binary = Some(fixture.directory.path().join("absent"));
        let failed = engine.spawn(missing).await.unwrap();
        let result = finished(&engine, &failed).await;
        assert!(matches!(
            result.result.as_ref().unwrap().status,
            Status::Failed
        ));
        assert!(result.result.as_ref().unwrap().error.is_some());
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_all_workers_and_drop_also_requests_cleanup() {
    let first = Fixture::new(Provider::Claude);
    let second = Fixture::new(Provider::Codex);
    let engine = engine();
    let a = engine.spawn(first.assignment("hold")).await.unwrap();
    let b = engine.spawn(second.assignment("hold")).await.unwrap();
    running(&engine, &a).await;
    running(&engine, &b).await;
    let (one, two) = tokio::join!(engine.shutdown(), engine.shutdown());
    one.unwrap();
    two.unwrap();
    assert_eq!(engine.inspect(&a).unwrap().state, RunState::Finished);
    assert_eq!(engine.inspect(&b).unwrap().state, RunState::Finished);
    reaped(first.pid()).await;
    reaped(second.pid()).await;
    let dropping = self::engine();
    let run = dropping.spawn(first.assignment("hold")).await.unwrap();
    running(&dropping, &run).await;
    let pid = first.pid();
    drop(dropping);
    reaped(pid).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interruption_during_native_startup_does_not_claim_early_settlement() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        fs::write(fixture.directory.path().join("hold-open"), "hold").unwrap();
        let engine = engine();
        let run = engine.spawn(fixture.assignment("never run")).await.unwrap();
        exists(fixture.directory.path().join("open-waiting")).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), engine.interrupt(&run))
                .await
                .is_err()
        );
        assert_ne!(engine.inspect(&run).unwrap().state, RunState::Finished);
        fs::write(fixture.directory.path().join("release-open"), "go").unwrap();
        let result = finished(&engine, &run).await;
        assert!(
            matches!(result.result.as_ref().unwrap().status, Status::Cancelled),
            "{result:?}"
        );
        // Codex waits for initialization; Claude may queue the turn before init.
        if provider == Provider::Codex {
            assert!(!fixture.input().contains("never run"));
        }
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[test]
fn constructing_an_engine_requires_a_runtime_and_valid_limits() {
    assert_eq!(
        Engine::new(Limits::default()).err().unwrap().kind,
        ErrorKind::Runtime
    );
    assert_eq!(
        Engine::new(Limits {
            max_runs: 0,
            ..Limits::default()
        })
        .err()
        .unwrap()
        .kind,
        ErrorKind::InvalidInput
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_failure_is_result_data_and_can_be_followed_by_a_successful_turn() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let run = engine.spawn(fixture.assignment("fail")).await.unwrap();
        let result = finished(&engine, &run).await;
        assert!(
            matches!(result.result.as_ref().unwrap().status, Status::Failed),
            "{result:?}"
        );
        assert!(result.result.as_ref().unwrap().error.is_some());
        if provider == Provider::Claude {
            assert!(
                result
                    .result
                    .as_ref()
                    .unwrap()
                    .text
                    .contains("partial answer")
            );
        }
        let correction = engine
            .follow_up(result.session, FollowUp::new("fix"))
            .await
            .unwrap();
        assert!(matches!(
            finished(&engine, &correction)
                .await
                .result
                .as_ref()
                .unwrap()
                .status,
            Status::Succeeded
        ));
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_resume_identity_fails_without_publishing_the_wrong_history() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let run = engine.spawn(fixture.assignment("first")).await.unwrap();
        let mut saved = finished(&engine, &run).await.saved_session.unwrap();
        engine.shutdown().await.unwrap();
        saved.native_id.push_str("-wrong");
        let next = self::engine();
        let run = next
            .follow_up(saved.clone(), FollowUp::new("resume mismatch"))
            .await
            .unwrap();
        let result = finished(&next, &run).await;
        assert_eq!(
            result.result.as_ref().unwrap().error.as_ref().unwrap().kind,
            RunErrorKind::Protocol,
            "{result:?}"
        );
        assert_eq!(result.saved_session.unwrap().native_id, saved.native_id);
        next.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_exit_between_turns_rejects_followup_without_rewriting_a_completed_result() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let run = engine.spawn(fixture.assignment("first")).await.unwrap();
        let completed = finished(&engine, &run).await;
        let before = serde_json::to_value(&completed).unwrap();
        assert_eq!(unsafe { libc::kill(fixture.pid(), libc::SIGTERM) }, 0);
        reaped(fixture.pid()).await;
        assert_eq!(
            engine
                .follow_up(completed.session, FollowUp::new("cannot run"))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::UnknownSession
        );
        assert_eq!(
            serde_json::to_value(engine.inspect(&run).unwrap()).unwrap(),
            before
        );
        // Nonzero native exit may also make explicit shutdown fail.
        let _ = engine.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleanup_failure_remains_sticky_and_does_not_rewrite_a_successful_assignment() {
    let fixture = Fixture::new(Provider::Codex);
    let path = fixture.directory.path().join("native");
    let source = fs::read_to_string(&path).unwrap().replace(
        "trap 'touch \"$DUE_SESSION_FIXTURE_DIR/exited\"' EXIT",
        "trap 'touch \"$DUE_SESSION_FIXTURE_DIR/exited\"; exit 17' EXIT",
    );
    fs::write(path, source).unwrap();
    let engine = engine();
    let run = engine.spawn(fixture.assignment("first")).await.unwrap();
    let completed = finished(&engine, &run).await;
    let error = engine.shutdown().await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Settlement);
    assert_eq!(engine.shutdown().await.unwrap_err(), error);
    assert!(matches!(
        completed.result.as_ref().unwrap().status,
        Status::Succeeded
    ));
    assert!(matches!(
        engine
            .inspect(&run)
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .status,
        Status::Succeeded
    ));
    reaped(fixture.pid()).await;
}

#[tokio::test]
async fn hostile_limits_inputs_and_ephemeral_resume_are_rejected_before_execution() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Engine>();
    send_sync::<RunSnapshot>();
    let fixture = Fixture::new(Provider::Claude);
    assert_eq!(
        Engine::new(Limits {
            settlement_timeout: Duration::MAX,
            ..Limits::default()
        })
        .err()
        .unwrap()
        .kind,
        ErrorKind::InvalidInput
    );
    let engine = engine();
    let mut assignment = fixture.assignment("valid");
    assignment.timeout = Some(Duration::MAX);
    assert_eq!(
        engine.spawn(assignment).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        engine
            .spawn(fixture.assignment(&"x".repeat(1024 * 1024 + 1)))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    let mut assignment = fixture.assignment("valid");
    assignment.worker.model = Some("x".repeat(1024 * 1024 + 1));
    assert_eq!(
        engine.spawn(assignment).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    let mut saved = SavedSession {
        native_id: "id".into(),
        worker: fixture.options(),
    };
    saved.worker.ephemeral = true;
    assert_eq!(
        engine
            .follow_up(saved, FollowUp::new("invalid"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert!(!fixture.directory.path().join("pid").exists());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_before_the_owner_is_scheduled_never_launches_a_child() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let run = engine
            .spawn(fixture.assignment("never start"))
            .await
            .unwrap();
        let result = engine.interrupt(&run).await.unwrap();
        assert!(matches!(
            result.result.as_ref().unwrap().status,
            Status::Cancelled
        ));
        assert!(!fixture.directory.path().join("pid").exists());
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_completion_does_not_release_a_pending_interrupt_acknowledgement() {
    for reject in [false, true] {
        let fixture = Fixture::new(Provider::Codex);
        let engine = Arc::new(engine());
        let prompt = if reject { "late-error" } else { "late-ack" };
        let run = engine.spawn(fixture.assignment(prompt)).await.unwrap();
        running(&engine, &run).await;
        let target = run.clone();
        let caller = engine.clone();
        let interrupt = tokio::spawn(async move { caller.interrupt(&target).await });
        exists(fixture.directory.path().join("interrupt-seen")).await;
        assert!(!interrupt.is_finished());
        assert_ne!(engine.inspect(&run).unwrap().state, RunState::Finished);
        assert_eq!(
            engine
                .follow_up(
                    engine.inspect(&run).unwrap().session,
                    FollowUp::new("too early")
                )
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Busy
        );
        fs::write(fixture.directory.path().join("release-ack"), "release").unwrap();
        let result = interrupt.await.unwrap().unwrap();
        let result = result.result.unwrap();
        if reject {
            assert_eq!(result.error.as_ref().unwrap().kind, RunErrorKind::Protocol);
            assert!(
                result
                    .error
                    .as_ref()
                    .unwrap()
                    .message
                    .contains("fixture late interrupt rejection"),
                "{result:?}"
            );
        } else {
            assert!(matches!(result.status, Status::Cancelled), "{result:?}");
        }
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn racing_followups_admit_exactly_one_turn_for_a_session() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let run = engine.spawn(fixture.assignment("first")).await.unwrap();
        let session = finished(&engine, &run).await.session;
        let (one, two) = tokio::join!(
            engine.follow_up(session.clone(), FollowUp::new("hold")),
            engine.follow_up(session, FollowUp::new("hold"))
        );
        let admitted = match (one, two) {
            (Ok(run), Err(error)) | (Err(error), Ok(run)) => {
                assert_eq!(error.kind, ErrorKind::Busy);
                run
            }
            other => panic!("follow-up reservation was not exclusive: {other:?}"),
        };
        engine.interrupt(&admitted).await.unwrap();
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_during_blocked_startup_reports_pending_settlement_and_can_be_retried() {
    let fixture = Fixture::new(Provider::Codex);
    fs::write(fixture.directory.path().join("hold-open"), "hold").unwrap();
    let engine = Engine::new(Limits {
        settlement_timeout: Duration::from_millis(50),
        ..Limits::default()
    })
    .unwrap();
    let run = engine
        .spawn(fixture.assignment("not dispatched"))
        .await
        .unwrap();
    exists(fixture.directory.path().join("open-waiting")).await;
    let pending = engine.shutdown().await;
    let snapshot = engine.inspect(&run).unwrap();
    // Always release the fixture before assertions so even a failure can settle.
    fs::write(fixture.directory.path().join("release-open"), "release").unwrap();
    assert_eq!(pending.unwrap_err().kind, ErrorKind::Settlement);
    assert_ne!(snapshot.state, RunState::Finished);
    let result = finished(&engine, &run).await;
    assert!(
        matches!(result.result.as_ref().unwrap().status, Status::Cancelled),
        "{result:?}"
    );
    engine.shutdown().await.unwrap();
    reaped(fixture.pid()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_workers_in_the_same_workspace_do_not_resume_each_others_context() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let first = engine
            .spawn(fixture.assignment("first fresh"))
            .await
            .unwrap();
        let one = finished(&engine, &first).await;
        let first_pid = fixture.pid();
        let second = engine
            .spawn(fixture.assignment("second fresh"))
            .await
            .unwrap();
        let two = finished(&engine, &second).await;
        let second_pid = fixture.pid();
        assert_eq!(one.result.as_ref().unwrap().text, "turn 1");
        assert_eq!(two.result.as_ref().unwrap().text, "turn 1");
        assert_ne!(one.session, two.session);
        assert_ne!(
            one.saved_session.unwrap().native_id,
            two.saved_session.unwrap().native_id
        );
        assert_ne!(first_pid, second_pid);
        engine.shutdown().await.unwrap();
        reaped(first_pid).await;
        reaped(second_pid).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_shutdown_future_does_not_abandon_its_native_cleanup() {
    let fixture = Fixture::new(Provider::Codex);
    fs::write(fixture.directory.path().join("hold-open"), "hold").unwrap();
    let engine = engine();
    let run = engine
        .spawn(fixture.assignment("not dispatched"))
        .await
        .unwrap();
    exists(fixture.directory.path().join("open-waiting")).await;
    let pending = tokio::time::timeout(Duration::from_millis(20), engine.shutdown()).await;
    fs::write(fixture.directory.path().join("release-open"), "release").unwrap();
    assert!(pending.is_err());
    let result = finished(&engine, &run).await;
    assert!(matches!(
        result.result.as_ref().unwrap().status,
        Status::Cancelled
    ));
    reaped(fixture.pid()).await;
    assert_eq!(
        engine
            .spawn(fixture.assignment("closed"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Closed
    );
    engine.shutdown().await.unwrap();
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_darwin_open_can_be_cancelled_without_waiting_for_another_engine() {
    let held = Fixture::new(Provider::Codex);
    let queued = Fixture::new(Provider::Claude);
    fs::write(held.directory.path().join("hold-open"), "hold").unwrap();
    let first = engine();
    let one = first.spawn(held.assignment("first")).await.unwrap();
    exists(held.directory.path().join("open-waiting")).await;
    let second = engine();
    let two = second
        .spawn(queued.assignment("never start"))
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), second.interrupt(&two)).await;
    let launched = queued.directory.path().join("pid").exists();
    fs::write(held.directory.path().join("release-open"), "go").unwrap();
    let result = result
        .expect("queued cancellation waited for the unrelated native open")
        .unwrap();
    assert!(matches!(
        result.result.as_ref().unwrap().status,
        Status::Cancelled
    ));
    assert!(!launched);
    finished(&first, &one).await;
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    reaped(held.pid()).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_utf8_alias_to_a_non_utf8_workspace_is_rejected_before_native_launch() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let fixture = Fixture::new(Provider::Claude);
    let target = fixture
        .directory
        .path()
        .join(OsString::from_vec(b"invalid-\xff".to_vec()));
    fs::create_dir(&target).unwrap();
    let alias = fixture.directory.path().join("alias");
    std::os::unix::fs::symlink(target, &alias).unwrap();
    let engine = engine();
    let mut assignment = fixture.assignment("valid");
    assignment.worker.cwd = alias;
    assert_eq!(
        engine.spawn(assignment).await.unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert!(!fixture.directory.path().join("pid").exists());
    engine.shutdown().await.unwrap();
}
