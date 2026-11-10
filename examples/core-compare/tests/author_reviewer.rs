//! Offline walkthrough: one issue, two fresh workers, one revision, and cleanup.
#![cfg(unix)]

mod support;

use std::{error::Error, path::Path, time::Duration};

use due_core::{
    Assignment, Engine, FollowUp, InteractionMode, Limits, Provider, RunRef, RunSnapshot,
    WaitOptions, WorkerOptions, result::Status,
};
use support::fake_provider;

const TURN_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn author_and_reviewer_keep_separate_histories() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let author_dir = directory.path().join("author");
    let reviewer_dir = directory.path().join("reviewer");
    let author = worker(Provider::Claude, &workspace, &author_dir);
    let reviewer = worker(Provider::Codex, &workspace, &reviewer_dir);

    let engine = Engine::new(Limits::default()).unwrap();
    let outcome = review_once(&engine, author, reviewer).await;
    // Always settle both workers before propagating a workflow error.
    engine.shutdown().await.unwrap();
    let [draft, review, revision, rereview] = outcome.unwrap();

    assert_ne!(draft.session, review.session);
    assert_eq!(draft.session, revision.session);
    assert_eq!(review.session, rereview.session);
    assert_ne!(native_id(&draft), native_id(&review));
    assert_eq!(native_id(&draft), native_id(&revision));
    assert_eq!(native_id(&review), native_id(&rereview));
    for (role, snapshot, expected) in [
        ("author", draft, "turn 1"),
        ("reviewer", review, "turn 1"),
        ("author", revision, "turn 2"),
        ("reviewer", rereview, "turn 2"),
    ] {
        let answer = &snapshot.result.unwrap().text;
        assert_eq!(answer, expected);
        println!("{role}: {answer} ({:?})", snapshot.session);
    }
    assert!(author_dir.join("exited").is_file());
    assert!(reviewer_dir.join("exited").is_file());
    println!("Both workers closed.");
}

fn worker(provider: Provider, workspace: &Path, fixture: &Path) -> WorkerOptions {
    let mut worker = WorkerOptions::new(provider, workspace);
    worker.binary = Some(fake_provider(fixture));
    worker.interaction_mode = InteractionMode::Reject;
    worker
}

async fn review_once(
    engine: &Engine,
    author: WorkerOptions,
    reviewer: WorkerOptions,
) -> Result<[RunSnapshot; 4], Box<dyn Error>> {
    let issue = "Issue: accept zero as a valid retry limit.";
    let draft = Assignment {
        worker: author,
        prompt: format!("{issue} Implement the fix and add a focused test."),
        timeout: Some(TURN_TIMEOUT),
    };
    let draft = finished(engine, engine.spawn(draft).await?).await?;

    // In real work the caller reads the diff from the workspace. These inputs
    // stand in for artifacts and findings because the fixtures only count turns.
    let review = Assignment {
        worker: reviewer,
        prompt: format!(
            "{issue} Review this diff independently. Diff summary: zero is accepted, but the test only covers positive limits."
        ),
        timeout: Some(TURN_TIMEOUT),
    };
    let review = finished(engine, engine.spawn(review).await?).await?;

    let revision = engine
        .follow_up(
            draft.session.clone(),
            follow_up("Review finding: add a regression test for a zero retry limit."),
        )
        .await?;
    let revision = finished(engine, revision).await?;

    let rereview = engine
        .follow_up(
            review.session.clone(),
            follow_up("Updated diff: a zero-limit regression test was added. Recheck the finding."),
        )
        .await?;
    let rereview = finished(engine, rereview).await?;
    Ok([draft, review, revision, rereview])
}

fn follow_up(prompt: &str) -> FollowUp {
    let mut assignment = FollowUp::new(prompt);
    assignment.timeout = Some(TURN_TIMEOUT);
    assignment
}

async fn finished(engine: &Engine, run: RunRef) -> Result<RunSnapshot, Box<dyn Error>> {
    let waited = engine.wait(&[run], WaitOptions::default()).await?;
    let snapshot = waited
        .runs
        .into_iter()
        .next()
        .ok_or("missing run snapshot")?;
    let result = snapshot.result.as_ref().ok_or("missing terminal result")?;
    if !matches!(result.status, Status::Succeeded) {
        return Err(format!("worker turn failed: {:?}", result.error).into());
    }
    Ok(snapshot)
}

fn native_id(snapshot: &RunSnapshot) -> &str {
    &snapshot.saved_session.as_ref().unwrap().native_id
}
