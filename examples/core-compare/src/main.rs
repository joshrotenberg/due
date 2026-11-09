//! Run the same prompt and follow-up in independent Claude and Codex sessions.

use std::{error::Error, path::Path, time::Duration};

use due_core::{
    Assignment, Engine, FollowUp, InteractionMode, Limits, Provider, RunRef, SessionStream,
    WaitOptions, WorkerOptions, result::Status,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let [prompt]: [String; 1] = std::env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "usage: due-core-compare <prompt>")?;
    let cwd = std::env::current_dir()?;
    let engine = Engine::new(Limits::default())?;
    let (claude, codex) = tokio::join!(
        compare(&engine, Provider::Claude, &cwd, &prompt),
        compare(&engine, Provider::Codex, &cwd, &prompt),
    );

    // Close both workers even if one comparison failed. The streams can still
    // replay retained evidence after the provider processes have exited.
    engine.shutdown().await?;
    for (provider, result) in [(Provider::Claude, claude), (Provider::Codex, codex)] {
        let mut items = result?;
        let mut count = 0;
        while items.next().await?.is_some() {
            count += 1;
        }
        println!("{} replayed {count} session items", provider.as_str());
    }
    Ok(())
}

async fn compare(
    engine: &Engine,
    provider: Provider,
    cwd: &Path,
    prompt: &str,
) -> Result<SessionStream, Box<dyn Error>> {
    let mut worker = WorkerOptions::new(provider, cwd);
    worker.interaction_mode = InteractionMode::Reject;
    worker.binary = std::env::var_os(match provider {
        Provider::Claude => "COMPARE_CLAUDE_BINARY",
        Provider::Codex => "COMPARE_CODEX_BINARY",
    })
    .map(Into::into);

    let mut assignment = Assignment::new(worker, prompt);
    assignment.timeout = Some(Duration::from_secs(120));
    let first = engine.spawn(assignment).await?;
    let session = engine.inspect(&first)?.session;
    let items = engine.subscribe(&session, 0)?;
    print_answer(engine, &first, provider, 1).await?;

    let mut follow_up = FollowUp::new("Summarize your previous answer in one sentence.");
    follow_up.timeout = Some(Duration::from_secs(120));
    let second = engine.follow_up(session, follow_up).await?;
    print_answer(engine, &second, provider, 2).await?;
    Ok(items)
}

async fn print_answer(
    engine: &Engine,
    run: &RunRef,
    provider: Provider,
    turn: u8,
) -> Result<(), Box<dyn Error>> {
    let waited = engine
        .wait(std::slice::from_ref(run), WaitOptions::default())
        .await?;
    let result = waited.runs[0]
        .result
        .as_ref()
        .ok_or("missing terminal result")?;
    if !matches!(result.status, Status::Succeeded) {
        let message = result
            .error
            .as_ref()
            .map_or("turn failed", |error| &error.message);
        return Err(format!("{} turn {turn}: {message}", provider.as_str()).into());
    }
    println!("{} turn {turn}: {}", provider.as_str(), result.text);
    Ok(())
}
