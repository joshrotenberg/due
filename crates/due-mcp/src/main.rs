use std::{path::PathBuf, sync::Arc, time::Duration};

use due_core::{
    Assignment, Engine, FollowUp, InteractionRef, Limits, Provider, RunRef, SessionRef,
    WaitOptions, WaitUntil, WorkerOptions,
};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_mcp::{
    BoxError, CallToolResult, McpRouter, StdioTransport, ToolBuilder,
    extract::{Json, State},
};

struct App {
    engine: Engine,
    cwd: PathBuf,
    claude_binary: Option<PathBuf>,
    codex_binary: Option<PathBuf>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Spawn {
    #[schemars(schema_with = "provider_schema")]
    provider: Provider,
    prompt: String,
    /// Optional execution deadline in milliseconds, including native startup.
    timeout_ms: Option<u64>,
}

fn provider_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "string", "enum": [Provider::Claude.as_str(), Provider::Codex.as_str()]})
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Continue {
    /// Live session from a previous spawn in this server process.
    #[schemars(with = "String")]
    session: SessionRef,
    prompt: String,
    /// Optional execution deadline in milliseconds.
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Run {
    #[schemars(with = "String")]
    run: RunRef,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Wait {
    #[schemars(with = "Vec<String>")]
    runs: Vec<RunRef>,
    /// Return once any supplied run finishes. Defaults to waiting for all.
    #[serde(default)]
    any: bool,
    /// Observation budget in milliseconds. Defaults to 1000; zero polls.
    #[schemars(range(max = 10000))]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Respond {
    /// Exact token from inspect.interactions[].id or read_items.items[].interaction.
    #[schemars(with = "serde_json::Map<String, Value>")]
    interaction: InteractionRef,
    /// Provider-native response object. Review the request before responding.
    response: serde_json::Map<String, Value>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Read {
    #[schemars(with = "String")]
    session: SessionRef,
    /// Exclusive cursor, or zero for the beginning of the journal.
    #[serde(default)]
    after: u64,
    /// Maximum number of items in this page. Defaults to 32.
    #[schemars(range(min = 1, max = 256))]
    limit: Option<usize>,
}

fn router(app: Arc<App>) -> McpRouter {
    McpRouter::new()
        .server_info("due-mcp", env!("CARGO_PKG_VERSION"))
        .instructions("Spawn a worker, then use its run and session references to inspect, wait, follow up, or read items. Use inspect to find pending interactions and respond with an explicit provider-native decision.")
        .tool(
            ToolBuilder::new("spawn")
                .description("Start a Claude or Codex worker in the server's working directory.")
                .extractor_handler(app.clone(), |State(app): State<Arc<App>>, Json(input): Json<Spawn>| async move {
                    let mut worker = WorkerOptions::new(input.provider, &app.cwd);
                    worker.binary = match input.provider {
                        Provider::Claude => app.claude_binary.clone(),
                        Provider::Codex => app.codex_binary.clone(),
                    };
                    let assignment = Assignment {
                        worker,
                        prompt: input.prompt,
                        timeout: input.timeout_ms.map(Duration::from_millis),
                    };
                    outcome(app.engine.spawn(assignment).await.and_then(|run| app.engine.inspect(&run)))
                })
                .build(),
        )
        .tool(
            ToolBuilder::new("follow_up")
                .description("Start another turn in a live session after its previous run settles.")
                .extractor_handler(app.clone(), |State(app): State<Arc<App>>, Json(input): Json<Continue>| async move {
                    let follow_up = FollowUp {
                        prompt: input.prompt,
                        timeout: input.timeout_ms.map(Duration::from_millis),
                    };
                    outcome(app.engine.follow_up(input.session, follow_up).await.and_then(|run| app.engine.inspect(&run)))
                })
                .build(),
        )
        .tool(
            ToolBuilder::new("inspect")
                .description("Read a run snapshot, including any pending interactions and terminal result.")
                .read_only()
                .extractor_handler(app.clone(), |State(app): State<Arc<App>>, Json(input): Json<Run>| async move {
                    outcome(app.engine.inspect(&input.run))
                })
                .build(),
        )
        .tool(
            ToolBuilder::new("wait")
                .description("Observe completion of runs for up to 10 seconds. Expiry leaves the runs active.")
                .read_only()
                .extractor_handler(app.clone(), |State(app): State<Arc<App>>, Json(input): Json<Wait>| async move {
                    let timeout_ms = input.timeout_ms.unwrap_or(1000);
                    if timeout_ms > 10_000 {
                        return tool_error(json!({"kind":"invalid_input","message":"timeout_ms must be at most 10000"}));
                    }
                    let options = WaitOptions {
                        until: if input.any { WaitUntil::Any } else { WaitUntil::All },
                        timeout: Some(Duration::from_millis(timeout_ms)),
                    };
                    outcome(app.engine.wait(&input.runs, options).await)
                })
                .build(),
        )
        .tool(
            ToolBuilder::new("interrupt")
                .description("Interrupt this exact run and wait for its settlement.")
                .extractor_handler(app.clone(), |State(app): State<Arc<App>>, Json(input): Json<Run>| async move {
                    outcome(app.engine.interrupt(&input.run).await)
                })
                .build(),
        )
        .tool(
            ToolBuilder::new("respond")
                .description("Submit a provider-native decision to an exact pending interaction.")
                .extractor_handler(app.clone(), |State(app): State<Arc<App>>, Json(input): Json<Respond>| async move {
                    outcome(app.engine.respond(&input.interaction, Value::Object(input.response)).await)
                })
                .build(),
        )
        .tool(
            ToolBuilder::new("read_items")
                .description("Read an available page of full session items after a cursor. Use the returned cursor for the next page.")
                .read_only()
                .extractor_handler(app, |State(app): State<Arc<App>>, Json(input): Json<Read>| async move {
                    let mut reader = match app.engine.subscribe(&input.session, input.after) {
                        Ok(reader) => reader,
                        Err(error) => return outcome::<due_core::SessionPage>(Err(error)),
                    };
                    match reader.read_available(input.limit.unwrap_or(32)) {
                        Ok(page) => structured(page),
                        Err(error) => tool_error(serde_json::to_value(error)?),
                    }
                })
                .build(),
        )
}

fn structured(value: impl Serialize) -> tower_mcp::Result<CallToolResult> {
    let value = serde_json::to_value(value)?;
    let mut result = CallToolResult::text(serde_json::to_string(&value)?);
    result.structured_content = Some(value);
    Ok(result)
}

fn tool_error(error: Value) -> tower_mcp::Result<CallToolResult> {
    let mut result = structured(json!({"error": error}))?;
    result.is_error = true;
    Ok(result)
}

fn outcome<T: Serialize>(result: Result<T, due_core::Error>) -> tower_mcp::Result<CallToolResult> {
    match result {
        Ok(value) => structured(value),
        Err(error) => tool_error(json!({"kind":error.kind,"message":error.message})),
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let app = Arc::new(App {
        engine: Engine::new(Limits::default())?,
        cwd: std::env::current_dir()?,
        claude_binary: std::env::var_os("DUE_MCP_CLAUDE_BINARY").map(Into::into),
        codex_binary: std::env::var_os("DUE_MCP_CODEX_BINARY").map(Into::into),
    });
    let mut transport = StdioTransport::without_server_notifications(router(app.clone()))
        .max_concurrent_requests(16)
        .drain_timeout(Duration::from_secs(5));
    let handle = transport.handle();
    let serving = transport.run();
    tokio::pin!(serving);
    tokio::select! {
        result = &mut serving => {
            app.engine.shutdown().await?;
            result?;
        }
        () = handle.stopping() => {
            // EOF can arrive while a tool is still waiting on native work.
            let (cleanup, result) = tokio::join!(app.engine.shutdown(), serving);
            cleanup?;
            result?;
        }
    }
    Ok(())
}
