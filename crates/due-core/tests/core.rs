#![cfg(all(feature = "resident", unix))]

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};

use due_core::{
    result::{ErrorKind as RunErrorKind, Status},
    *,
};
use tempfile::TempDir;

struct Fixture {
    directory: TempDir,
    binary: PathBuf,
    provider: Provider,
}

#[test]
fn worker_preflight_resolves_only_the_selected_provider_without_launching_it() {
    for (provider, arguments) in [
        (
            Provider::Claude,
            &[
                "--restricted",
                "--strict-mcp-config",
                "--tools",
                "Read,Edit",
            ][..],
        ),
        (
            Provider::Codex,
            &[
                "-c",
                "sandbox_mode=\"read-only\"",
                "-c",
                "approval_policy=\"never\"",
            ][..],
        ),
    ] {
        let fixture = Fixture::new(provider);
        let mut options = fixture.options();
        options.native_args = arguments.iter().map(|value| (*value).into()).collect();
        let validated = options.clone().validate().unwrap();
        assert_eq!(validated.provider, provider);
        assert_eq!(
            validated.cwd,
            fixture.directory.path().canonicalize().unwrap()
        );
        assert_eq!(validated.native_args, arguments);
        options.native_args.push("--resume".into());
        assert_eq!(
            options.validate().unwrap_err().kind,
            ErrorKind::InvalidInput
        );
        assert!(!fixture.directory.path().join("argv").exists());
    }
}

impl Fixture {
    fn new(provider: Provider) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = include_str!("fixtures/session-agent.sh")
            .replace("native=\"native-$provider\"", r#"native="native-$provider-$$"
previous=
saved=
fork=0
for argument in "$@"; do
  if [ "$previous" = --resume ]; then saved="$argument"; fi
  if [ "$argument" = --fork-session ]; then fork=1; fi
  previous="$argument"
done
if [ -n "$saved" ] && [ -f "$DUE_SESSION_FIXTURE_DIR/history-$saved" ]; then
  count=$(cat "$DUE_SESSION_FIXTURE_DIR/history-$saved")
  if [ "$fork" = 0 ] || [ -f "$DUE_SESSION_FIXTURE_DIR/fork-alias" ]; then native="$saved"; fi
fi"#)
            .replace("count=$((count + 1))", "count=$((count + 1)); printf '%s' \"$count\" > \"$DUE_SESSION_FIXTURE_DIR/history-$native\"")
            .replace("        thread/start|thread/resume)", r#"        thread/fork)
          source_re='"threadId":"([^"]+)"'
          if [ -f "$DUE_SESSION_FIXTURE_DIR/fork-unsupported" ]; then
            emit '{"id":'"$id"',"error":{"code":-32601,"message":"thread/fork unsupported by fixture"}}'
          elif [[ "$line" =~ $source_re ]]; then
            requested="${BASH_REMATCH[1]}"
            count=$(cat "$DUE_SESSION_FIXTURE_DIR/history-$requested")
            if [ -f "$DUE_SESSION_FIXTURE_DIR/fork-alias" ]; then native="$requested"; fi
            ancestry="$requested"
            if [ -f "$DUE_SESSION_FIXTURE_DIR/fork-wrong-source" ]; then ancestry="wrong-source"; fi
            result "$id" '{"thread":{"id":"'"$native"'","forkedFromId":"'"$ancestry"'","turns":[]}}'
          fi
          ;;
        thread/start|thread/resume)
          resume_re='"threadId":"([^"]+)"'
          if [ "$method" = thread/resume ] && [[ "$line" =~ $resume_re ]]; then
            requested="${BASH_REMATCH[1]}"
            if [ -f "$DUE_SESSION_FIXTURE_DIR/history-$requested" ]; then
              native="$requested"
              count=$(cat "$DUE_SESSION_FIXTURE_DIR/history-$native")
            fi
          fi"#);
        let source = source.replace("            finish interrupted ''\n          elif", "            finish interrupted ''\n            : > \"$DUE_SESSION_FIXTURE_DIR/interrupt-seen\"\n            while [ ! -f \"$DUE_SESSION_FIXTURE_DIR/release-ack\" ]; do /bin/sleep 0.01; done\n          elif");
        // Reuse the same request ID within one native turn, after resolution.
        let source = source.replace("finish completed 'resolved before start'", r#"if [ -f "$DUE_SESSION_FIXTURE_DIR/reissue" ]; then
            : > "$DUE_SESSION_FIXTURE_DIR/resolved"
            while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-reissue" ]; do /bin/sleep 0.01; done
            emit '{"method":"item/commandExecution/requestApproval","id":17,"params":{"threadId":"'"$native"'","turnId":"'"$turn"'","command":"second generation"}}'
          else finish completed 'resolved before start'; fi"#);
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

#[tokio::test]
async fn release_reclaims_both_limits_without_affecting_another_worker() {
    let claude = Fixture::new(Provider::Claude);
    let codex = Fixture::new(Provider::Codex);
    let engine = Engine::new(Limits {
        max_sessions: 2,
        max_runs: 3,
        ..Limits::default()
    })
    .unwrap();

    let first = engine.spawn(claude.assignment("first")).await.unwrap();
    let first_snapshot = finished(&engine, &first).await;
    let session = first_snapshot.session;
    let mut reader = engine.subscribe(&session, 0).unwrap();
    let other = engine.spawn(codex.assignment("first")).await.unwrap();
    let other_snapshot = finished(&engine, &other).await;
    assert_eq!(
        engine
            .spawn(claude.assignment("first"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Capacity
    );

    let last = engine
        .follow_up(session.clone(), FollowUp::new("native-exit"))
        .await
        .unwrap();
    finished(&engine, &last).await;
    reaped(claude.pid()).await;
    assert_eq!(
        engine
            .follow_up(other_snapshot.session.clone(), FollowUp::new("second"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Capacity
    );

    engine.release_session(&session).await.unwrap();
    assert_eq!(
        engine.inspect(&first).unwrap_err().kind,
        ErrorKind::UnknownRun
    );
    assert_eq!(
        engine.inspect(&last).unwrap_err().kind,
        ErrorKind::UnknownRun
    );
    assert_eq!(
        engine.release_session(&session).await.unwrap_err().kind,
        ErrorKind::UnknownSession
    );
    assert!(
        matches!(engine.subscribe(&session, 0), Err(error) if error.kind == ErrorKind::UnknownSession)
    );
    let page = reader.read_available(256).unwrap();
    assert!(page.closed);
    assert!(page.items.iter().any(|item| item.kind == "run_finished"));

    let replacement = engine.spawn(claude.assignment("first")).await.unwrap();
    assert_ne!(finished(&engine, &replacement).await.session, session);
    let follow_up = engine
        .follow_up(other_snapshot.session, FollowUp::new("second"))
        .await
        .unwrap();
    let result = finished(&engine, &follow_up).await.result.unwrap();
    assert_eq!(result.text, "turn 2");
    assert!(engine.inspect(&other).is_ok());
    let error = engine
        .follow_up(session, FollowUp::new("stale"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownSession);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn confirmed_abnormal_exit_reclaims_session_and_run_capacity() {
    for provider in [Provider::Claude, Provider::Codex] {
        let crashed = Fixture::new(provider);
        let healthy = Fixture::new(Provider::Codex);
        let engine = Engine::new(Limits {
            max_sessions: 2,
            max_runs: 2,
            ..Limits::default()
        })
        .unwrap();
        let first = engine
            .spawn(crashed.assignment("native-exit-error"))
            .await
            .unwrap();
        let stopped = finished(&engine, &first).await;
        assert!(matches!(
            stopped.result.as_ref().unwrap().status,
            Status::Failed
        ));
        let mut reader = engine.subscribe(&stopped.session, 0).unwrap();
        let sibling = engine.spawn(healthy.assignment("first")).await.unwrap();
        let sibling = finished(&engine, &sibling).await;
        assert_eq!(sibling.result.as_ref().unwrap().text, "turn 1");
        assert_eq!(
            engine
                .spawn(crashed.assignment("fresh"))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Capacity
        );

        let released = engine
            .release_session_with_report(&stopped.session)
            .await
            .unwrap();
        let diagnostic = released.exit_error.unwrap();
        let expected = match provider {
            Provider::Claude => "exit status: 17",
            Provider::Codex => "exit code 17",
        };
        assert!(diagnostic.contains(expected), "{provider:?}: {diagnostic}");
        reaped(crashed.pid()).await;
        assert_eq!(
            engine.inspect(&first).unwrap_err().kind,
            ErrorKind::UnknownRun
        );
        let page = reader.read_available(256).unwrap();
        assert!(page.closed);
        assert!(page.items.iter().any(|item| {
            item.kind == "session_stream_closed"
                && item.data["error"]
                    .as_str()
                    .is_some_and(|error| error.contains(&diagnostic))
        }));

        // The only free session and run slots are from the crashed worker.
        let replacement = engine.spawn(crashed.assignment("fresh")).await.unwrap();
        let replacement = finished(&engine, &replacement).await;
        assert_eq!(replacement.result.as_ref().unwrap().text, "turn 1");
        engine.release_session(&replacement.session).await.unwrap();

        let next = engine
            .follow_up(sibling.session, FollowUp::new("second"))
            .await
            .unwrap();
        assert_eq!(
            finished(&engine, &next).await.result.as_ref().unwrap().text,
            "turn 2"
        );
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn release_reserves_the_session_and_survives_a_dropped_caller() {
    let fixture = Fixture::new(Provider::Claude);
    let engine = Arc::new(engine());
    let run = engine.spawn(fixture.assignment("hold")).await.unwrap();
    running(&engine, &run).await;
    let session = engine.inspect(&run).unwrap().session;
    assert_eq!(
        engine.release_session(&session).await.unwrap_err().kind,
        ErrorKind::Busy
    );
    engine.interrupt(&run).await.unwrap();

    let dir = fixture.directory.path();
    fs::write(dir.join("hold-close"), "").unwrap();
    let caller = tokio::spawn({
        let engine = Arc::clone(&engine);
        let session = session.clone();
        async move { engine.release_session(&session).await }
    });
    exists(dir.join("close-waiting")).await;
    assert_eq!(
        engine.release_session(&session).await.unwrap_err().kind,
        ErrorKind::Busy
    );
    assert_eq!(
        engine
            .follow_up(session.clone(), FollowUp::new("second"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Busy
    );
    assert!(engine.inspect(&run).is_ok());

    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    fs::write(dir.join("release-close"), "").unwrap();
    reaped(fixture.pid()).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while engine.inspect(&run).is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn release_keeps_capacity_reserved_if_settlement_times_out() {
    let fixture = Fixture::new(Provider::Claude);
    let engine = Engine::new(Limits {
        max_sessions: 1,
        max_runs: 2,
        settlement_timeout: Duration::from_millis(100),
    })
    .unwrap();
    let run = engine.spawn(fixture.assignment("first")).await.unwrap();
    let session = finished(&engine, &run).await.session;
    let dir = fixture.directory.path();
    fs::write(dir.join("hold-close"), "").unwrap();

    let result = engine.release_session(&session).await;
    fs::write(dir.join("release-close"), "").unwrap();
    reaped(fixture.pid()).await;
    assert_eq!(result.unwrap_err().kind, ErrorKind::Settlement);
    assert!(engine.inspect(&run).is_ok());
    assert!(
        engine
            .subscribe(&session, 0)
            .unwrap()
            .read_available(256)
            .unwrap()
            .closed
    );
    assert_eq!(
        engine
            .follow_up(session, FollowUp::new("second"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::UnknownSession
    );
    let error = engine.spawn(fixture.assignment("fresh")).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Capacity);
    assert_eq!(error.message, "worker limit reached");
    assert_eq!(
        engine.shutdown().await.unwrap_err().kind,
        ErrorKind::Settlement
    );
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
        let mut assignment = fixture.assignment("ask");
        assignment.worker.interaction_mode = InteractionMode::Reject;
        let ask = engine.spawn(assignment).await.unwrap();
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
async fn fork_branches_saved_history_and_preserves_the_source() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let source = engine.spawn(fixture.assignment("first")).await.unwrap();
        let original = finished(&engine, &source).await;
        let before = serde_json::to_value(&original).unwrap();
        let saved = original.saved_session.unwrap();
        let source_pid = fixture.pid();
        let run = engine
            .fork(saved.clone(), FollowUp::new("branch"))
            .await
            .unwrap();
        let fork = finished(&engine, &run).await;
        let fork_pid = fixture.pid();
        let branch = fork.saved_session.unwrap();
        assert_ne!(fork.session, original.session);
        assert_ne!(branch.native_id, saved.native_id);
        assert_eq!(branch.worker.cwd, saved.worker.cwd);
        assert_eq!(branch.worker.provider, saved.worker.provider);
        assert_eq!(fork.result.unwrap().text, "turn 2");

        let mut stream = engine.subscribe(&fork.session, 0).unwrap();
        let page = stream.read_available(256).unwrap();
        assert!(page.items.iter().any(|item| {
            item.kind == "session_opened"
                && item.data["resolved_options"]["fork_source"] == saved.native_id
        }));
        if provider == Provider::Codex {
            assert!(page.items.iter().any(|item| {
                item.data["method"] == "thread/fork"
                    && item.data["response"]["thread"]["forkedFromId"] == saved.native_id
            }));
        }

        let child_next = engine
            .follow_up(fork.session, FollowUp::new("branch again"))
            .await
            .unwrap();
        let parent_next = engine
            .follow_up(original.session, FollowUp::new("source again"))
            .await
            .unwrap();
        assert_eq!(
            finished(&engine, &child_next).await.result.unwrap().text,
            "turn 3"
        );
        assert_eq!(
            finished(&engine, &parent_next).await.result.unwrap().text,
            "turn 2"
        );
        assert_eq!(
            serde_json::to_value(engine.inspect(&source).unwrap()).unwrap(),
            before
        );
        engine.shutdown().await.unwrap();
        reaped(source_pid).await;
        reaped(fork_pid).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_rejects_an_alias_and_retains_native_unsupported_errors() {
    let fixture = Fixture::new(Provider::Claude);
    let invalid = SavedSession {
        native_id: "--continue".into(),
        worker: fixture.options(),
    };
    assert_eq!(
        engine()
            .fork(invalid, FollowUp::new("branch"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
    assert!(!fixture.directory.path().join("pid").exists());
    for (provider, mode, expected) in [
        (
            Provider::Claude,
            "fork-alias",
            "reused the source session identity",
        ),
        (
            Provider::Codex,
            "fork-alias",
            "reused the source session identity",
        ),
        (
            Provider::Codex,
            "fork-wrong-source",
            "unexpected source ancestry",
        ),
        (
            Provider::Codex,
            "fork-unsupported",
            "thread/fork unsupported by fixture",
        ),
    ] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let saved = SavedSession {
            native_id: "fixture-source".into(),
            worker: fixture.options(),
        };
        let root = fixture.directory.path();
        fs::write(root.join("history-fixture-source"), "1").unwrap();
        fs::write(root.join(mode), "").unwrap();
        let run = engine.fork(saved, FollowUp::new("branch")).await.unwrap();
        let snapshot = finished(&engine, &run).await;
        assert!(snapshot.saved_session.is_none());
        let result = snapshot.result.unwrap();
        assert!(result.session_id.is_none());
        let error = result.error.as_ref().unwrap();
        assert!(error.message.contains(expected), "{error:?}");
        if provider == Provider::Codex {
            assert!(!fixture.input().contains(r#""method":"turn/start""#));
        }
        let _ = engine.shutdown().await;
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
        let mut items = engine.subscribe(&completed.session, 0).unwrap();
        assert_eq!(unsafe { libc::kill(fixture.pid(), libc::SIGTERM) }, 0);
        reaped(fixture.pid()).await;
        // Reaping can precede the session owner recording the failure.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let item = items
                    .next()
                    .await
                    .unwrap()
                    .expect("session stream closed without reporting the exit");
                if item.kind == "session_failed" {
                    break;
                }
            }
        })
        .await
        .expect("session owner did not report the exit");
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
async fn shutdown_closes_idle_siblings_when_startup_is_stuck() {
    let fixture = Fixture::new(Provider::Codex);
    let idle = Fixture::new(Provider::Codex);
    let engine = Engine::new(Limits {
        settlement_timeout: Duration::from_millis(500),
        ..Limits::default()
    })
    .unwrap();
    let idle_run = engine.spawn(idle.assignment("finish")).await.unwrap();
    let idle_snapshot = finished(&engine, &idle_run).await;
    assert!(matches!(
        idle_snapshot.result.unwrap().status,
        Status::Succeeded
    ));

    fs::write(fixture.directory.path().join("hold-open"), "hold").unwrap();
    let run = engine
        .spawn(fixture.assignment("not dispatched"))
        .await
        .unwrap();
    exists(fixture.directory.path().join("open-waiting")).await;
    let pending = engine.shutdown().await;
    let snapshot = engine.inspect(&run).unwrap();
    let idle_exited = idle.directory.path().join("exited").is_file();
    let mut reader = engine.subscribe(&idle_snapshot.session, 0).unwrap();
    let idle_closed = reader.read_available(256).unwrap().closed;
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
    reaped(idle.pid()).await;
    assert!(
        idle_exited,
        "stalled startup prevented idle sibling cleanup"
    );
    assert!(idle_closed, "idle sibling's journal did not close");
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
    let result = tokio::time::timeout(Duration::from_secs(1), second.interrupt(&two)).await;
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

#[tokio::test]
async fn shutdown_closes_admission_before_its_cleanup_owner_is_scheduled() {
    use std::{future::Future, task::Poll};
    let fixture = Fixture::new(Provider::Claude);
    let engine = engine();
    let shutdown = engine.shutdown();
    tokio::pin!(shutdown);
    std::future::poll_fn(|context| {
        assert!(shutdown.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    // On this current-thread runtime no spawned owner has run yet.
    assert_eq!(
        engine
            .spawn(fixture.assignment("closed"))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Closed
    );
    shutdown.await.unwrap();
    assert!(!fixture.directory.path().join("pid").exists());
}

async fn approval(engine: &Engine, run: &RunRef) -> InteractionRef {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = engine.inspect(run).unwrap();
            assert_ne!(snapshot.state, RunState::Finished, "{snapshot:?}");
            if snapshot.state == RunState::AwaitingInput && !snapshot.interactions.is_empty() {
                return snapshot.interactions[0].id.clone();
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}
fn decision(provider: Provider, allow: bool) -> serde_json::Value {
    match (provider, allow) {
        (Provider::Claude, true) => serde_json::json!({"behavior":"allow"}),
        (Provider::Claude, false) => {
            serde_json::json!({"behavior":"deny","message":"caller declined"})
        }
        (Provider::Codex, true) => serde_json::json!({"decision":"accept"}),
        (Provider::Codex, false) => serde_json::json!({"decision":"decline"}),
    }
}
async fn stream_through(stream: &mut SessionStream, kind: &str) -> Vec<SessionItem> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut items = Vec::new();
        loop {
            let item = stream
                .next()
                .await
                .unwrap()
                .expect("premature stream closure");
            let found = item.kind == kind;
            items.push(item);
            if found {
                return items;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_approval_and_denial_preserve_live_session_and_raw_items() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let first = engine.spawn(fixture.assignment("ask")).await.unwrap();
        let session = engine.inspect(&first).unwrap().session;
        let mut stream = engine.subscribe(&session, 0).unwrap();
        let token = approval(&engine, &first).await;
        let items = stream_through(&mut stream, "interaction").await;
        let item = items.last().unwrap();
        assert_eq!(item.interaction.as_ref(), Some(&token));
        assert_eq!(item.run.as_ref(), Some(&first));
        assert!(item.data["request"].is_object());
        // Bad responses do not consume the pending generation.
        for bad in [serde_json::json!(null), serde_json::json!([])] {
            assert_eq!(
                engine.respond(&token, bad).await.unwrap_err().kind,
                ErrorKind::InvalidInput
            );
        }
        if provider == Provider::Claude {
            for bad in [
                serde_json::json!({"behavior":"allow","updated_input":[]}),
                serde_json::json!({"behavior":"allow","typo":true}),
            ] {
                assert_eq!(
                    engine.respond(&token, bad).await.unwrap_err().kind,
                    ErrorKind::InvalidInput
                );
            }
        }
        let other = self::engine();
        assert_eq!(
            other
                .respond(&token, decision(provider, true))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::UnknownRun
        );
        other.shutdown().await.unwrap();
        assert_eq!(
            engine
                .respond(&token, decision(provider, true))
                .await
                .unwrap()
                .interaction,
            token
        );
        let original = finished(&engine, &first).await;
        assert_eq!(original.result.as_ref().unwrap().text, "permission allowed");
        assert!(original.interactions.is_empty());
        assert_eq!(
            engine
                .respond(&token, decision(provider, true))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::UnknownInteraction
        );
        stream_through(&mut stream, "run_finished").await;
        let second = engine
            .follow_up(session.clone(), FollowUp::new("ask"))
            .await
            .unwrap();
        let second_token = approval(&engine, &second).await;
        assert_ne!(token, second_token); // Native request IDs are deliberately reused.
        assert_eq!(
            engine
                .respond(&token, decision(provider, true))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::UnknownInteraction
        );
        engine
            .respond(&second_token, decision(provider, false))
            .await
            .unwrap();
        assert_eq!(
            finished(&engine, &second)
                .await
                .result
                .as_ref()
                .unwrap()
                .text,
            "permission denied"
        );
        assert_eq!(original.result.as_ref().unwrap().text, "permission allowed");
        stream_through(&mut stream, "run_finished").await;
        engine.shutdown().await.unwrap();
        let tail = stream_through(&mut stream, "session_stream_closed").await;
        assert!(tail.last().unwrap().run.is_none());
        assert!(stream.next().await.unwrap().is_none());
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streams_replay_full_native_payloads_across_turns_with_independent_cursors() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let first = engine
            .spawn(fixture.assignment("partial-payload"))
            .await
            .unwrap();
        let session = engine.inspect(&first).unwrap().session;
        let mut a = engine.subscribe(&session, 0).unwrap();
        let mut b = engine.subscribe(&session, 0).unwrap();
        let items = stream_through(&mut a, "run_finished").await;
        assert_eq!(items[0].kind, "run_admitted");
        let opened = items
            .iter()
            .find(|item| item.kind == "session_opened")
            .unwrap();
        assert_eq!(opened.data["capabilities"]["session_stream"], true);
        let raw = serde_json::to_string(&items).unwrap();
        assert!(raw.contains("fixturePayload"), "{raw}");
        assert!(raw.contains("future"), "{raw}");
        assert!(raw.contains("line one\\nline two"), "{raw}");
        if provider == Provider::Claude {
            assert!(raw.contains("fixturePartial"), "{raw}");
            assert!(raw.contains("partial chunk\\nsecond line"), "{raw}");
        } else {
            assert!(raw.contains("fixtureResponse"), "{raw}");
            assert!(raw.contains("fixture/idle"), "{raw}");
        }
        assert!(items.iter().all(
            |item| item.session == session && item.run.as_ref().is_none_or(|run| run == &first)
        ));
        assert!(
            items
                .windows(2)
                .all(|pair| pair[1].cursor == pair[0].cursor + 1)
        );
        assert_eq!(
            stream_through(&mut b, "run_finished")
                .await
                .iter()
                .map(|i| i.cursor)
                .collect::<Vec<_>>(),
            items.iter().map(|i| i.cursor).collect::<Vec<_>>()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), a.next())
                .await
                .is_err()
        );
        assert_eq!(a.cursor(), items.last().unwrap().cursor);
        drop(b);
        let mut resumed = engine.subscribe(&session, a.cursor()).unwrap();
        let second = engine
            .follow_up(session.clone(), FollowUp::new("next"))
            .await
            .unwrap();
        let next = stream_through(&mut resumed, "run_finished").await;
        assert_eq!(next.first().unwrap().cursor, a.cursor() + 1);
        assert!(
            next.iter()
                .all(|item| item.run.as_ref().is_none_or(|run| run == &second))
        );
        assert_eq!(
            finished(&engine, &second)
                .await
                .result
                .as_ref()
                .unwrap()
                .text,
            "turn 2"
        );
        assert_eq!(
            engine.subscribe(&session, u64::MAX).err().unwrap().kind,
            ErrorKind::InvalidInput
        );
        engine.shutdown().await.unwrap();
        // Late readers retain history after manager close removed its read-side slot.
        let mut late = engine.subscribe(&session, 0).unwrap();
        let all = stream_through(&mut late, "session_stream_closed").await;
        assert_eq!(all.iter().filter(|i| i.kind == "run_finished").count(), 2);
        assert!(late.next().await.unwrap().is_none());
        let mut reader = engine.subscribe(&session, 0).unwrap();
        assert_eq!(
            reader.read_available(0).unwrap_err(),
            StreamError::InvalidLimit
        );
        assert_eq!(reader.cursor(), 0);
        let mut paged = Vec::new();
        loop {
            let page = reader.read_available(3).unwrap();
            assert!(page.closed);
            assert_eq!(page.latest_cursor, late.cursor());
            assert_eq!(page.cursor, reader.cursor());
            paged.extend(page.items);
            if page.cursor == page.latest_cursor {
                break;
            }
        }
        assert_eq!(
            serde_json::to_value(paged).unwrap(),
            serde_json::to_value(all).unwrap()
        );
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_pending_approvals_settles_without_granting_permission() {
    for provider in [Provider::Claude, Provider::Codex] {
        for shutdown in [false, true] {
            let fixture = Fixture::new(provider);
            let engine = engine();
            let run = engine.spawn(fixture.assignment("ask")).await.unwrap();
            let token = approval(&engine, &run).await;
            if shutdown {
                engine.shutdown().await.unwrap();
            } else {
                engine.interrupt(&run).await.unwrap();
            }
            let final_state = finished(&engine, &run).await;
            assert_eq!(
                final_state
                    .result
                    .as_ref()
                    .unwrap()
                    .error
                    .as_ref()
                    .unwrap()
                    .kind,
                RunErrorKind::Cancelled
            );
            assert!(final_state.interactions.is_empty());
            let error = engine
                .respond(&token, decision(provider, true))
                .await
                .unwrap_err();
            assert_eq!(
                error.kind,
                if shutdown {
                    ErrorKind::Closed
                } else {
                    ErrorKind::UnknownInteraction
                }
            );
            assert!(!fixture.input().contains("\"behavior\":\"allow\""));
            assert!(!fixture.input().contains("\"decision\":\"accept\""));
            engine.shutdown().await.unwrap();
            reaped(fixture.pid()).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_approval_submissions_deliver_at_most_once() {
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let run = engine.spawn(fixture.assignment("ask")).await.unwrap();
        let token = approval(&engine, &run).await;
        let (a, b) = tokio::join!(
            engine.respond(&token, decision(provider, true)),
            engine.respond(&token, decision(provider, true))
        );
        assert_ne!(a.is_ok(), b.is_ok());
        assert!(matches!(
            a.err().or_else(|| b.err()).unwrap().kind,
            ErrorKind::Busy | ErrorKind::UnknownInteraction
        ));
        assert_eq!(
            finished(&engine, &run).await.result.as_ref().unwrap().text,
            "permission allowed"
        );
        let key = if provider == Provider::Claude {
            "\"behavior\":\"allow\""
        } else {
            "\"decision\":\"accept\""
        };
        assert_eq!(
            fixture.input().lines().filter(|l| l.contains(key)).count(),
            1
        );
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolved_or_completed_requests_cannot_be_answered_from_replay() {
    for prompt in ["ask-resolved", "ask-completed"] {
        let fixture = Fixture::new(Provider::Codex);
        let engine = engine();
        let run = engine.spawn(fixture.assignment(prompt)).await.unwrap();
        let snapshot = finished(&engine, &run).await;
        let mut stream = engine.subscribe(&snapshot.session, 0).unwrap();
        let items = stream_through(&mut stream, "run_finished").await;
        let token = items.iter().find_map(|i| i.interaction.as_ref()).unwrap();
        assert_eq!(
            engine
                .respond(token, decision(Provider::Codex, true))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::UnknownInteraction
        );
        assert!(snapshot.interactions.is_empty());
        assert!(!fixture.input().contains("\"decision\":\"accept\""));
        let next = engine
            .follow_up(snapshot.session, FollowUp::new("next"))
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
async fn stream_preserves_admission_and_terminal_failure_when_native_open_fails() {
    let fixture = Fixture::new(Provider::Codex);
    fs::write(fixture.directory.path().join("native"), "exit 17\n").unwrap();
    let engine = engine();
    let run = engine.spawn(fixture.assignment("first")).await.unwrap();
    let snapshot = finished(&engine, &run).await;
    assert!(snapshot.result.as_ref().unwrap().error.is_some());
    let mut stream = engine.subscribe(&snapshot.session, 0).unwrap();
    let items = stream_through(&mut stream, "session_stream_closed").await;
    assert_eq!(items[0].kind, "run_admitted");
    assert!(
        items
            .iter()
            .any(|i| i.kind == "run_finished" && i.run.as_ref() == Some(&run))
    );
    assert!(stream.next().await.unwrap().is_none());
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_turn_reused_native_request_id_cannot_accept_an_old_generation() {
    let fixture = Fixture::new(Provider::Codex);
    fs::write(fixture.directory.path().join("reissue"), "").unwrap();
    let engine = engine();
    let run = engine
        .spawn(fixture.assignment("ask-resolved"))
        .await
        .unwrap();
    let session = engine.inspect(&run).unwrap().session;
    let mut stream = engine.subscribe(&session, 0).unwrap();
    let items = stream_through(&mut stream, "interaction").await;
    let old = items.last().unwrap().interaction.clone().unwrap();
    stream_through(&mut stream, "interaction_resolved").await;
    assert_eq!(
        engine
            .respond(&old, decision(Provider::Codex, true))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::UnknownInteraction
    );
    fs::write(fixture.directory.path().join("release-reissue"), "").unwrap();
    let items = stream_through(&mut stream, "interaction").await;
    let current = items.last().unwrap().interaction.clone().unwrap();
    assert_ne!(old, current);
    assert_eq!(
        engine
            .respond(&old, decision(Provider::Codex, true))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::UnknownInteraction
    );
    engine
        .respond(&current, decision(Provider::Codex, false))
        .await
        .unwrap();
    assert_eq!(
        finished(&engine, &run).await.result.as_ref().unwrap().text,
        "permission denied"
    );
    assert!(!fixture.input().contains("\"decision\":\"accept\""));
    engine.shutdown().await.unwrap();
    reaped(fixture.pid()).await;
}

#[tokio::test]
async fn dropping_a_submitted_response_future_keeps_delivery_ownership() {
    use std::{future::Future, task::Poll};
    for provider in [Provider::Claude, Provider::Codex] {
        let fixture = Fixture::new(provider);
        let engine = engine();
        let run = engine.spawn(fixture.assignment("ask")).await.unwrap();
        let token = approval(&engine, &run).await;
        let mut response = Box::pin(engine.respond(&token, decision(provider, false)));
        // On this current-thread runtime the independent owner cannot execute
        // until we yield, so this deterministically drops during submission.
        std::future::poll_fn(|cx| {
            assert!(response.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(response);
        assert_eq!(
            finished(&engine, &run).await.result.as_ref().unwrap().text,
            "permission denied"
        );
        engine.shutdown().await.unwrap();
        reaped(fixture.pid()).await;
    }
}
