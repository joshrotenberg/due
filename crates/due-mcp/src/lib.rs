//! Host Due workers over one MCP connection using newline-delimited I/O.

use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};

use due_core::{
    Assignment, Engine, FollowUp, InteractionRef, Limits, Provider, RunRef, SessionRef,
    WaitOptions, WaitUntil, WorkerOptions,
};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tower_mcp::{
    BoxError, CallToolResult, McpRouter, StdioPipes, StdioTransport, ToolBuilder,
    extract::{Json, State},
};

/// Host-controlled settings shared by the workers on one connection.
pub struct Config {
    /// Working directory for native workers.
    pub cwd: PathBuf,
    /// Claude executable override. The core otherwise resolves it from PATH.
    pub claude_binary: Option<PathBuf>,
    /// Codex executable override. The core otherwise resolves it from PATH.
    pub codex_binary: Option<PathBuf>,
    /// Resource limits for this connection's engine.
    pub limits: Limits,
}

impl Config {
    /// Start with native executable resolution and default engine limits.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            claude_binary: None,
            codex_binary: None,
            limits: Limits::default(),
        }
    }
}

struct App {
    engine: Engine,
    config: Config,
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
                    let mut worker = WorkerOptions::new(input.provider, &app.config.cwd);
                    worker.binary = match input.provider {
                        Provider::Claude => app.config.claude_binary.clone(),
                        Provider::Codex => app.config.codex_binary.clone(),
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

/// Serve one MCP connection on caller-supplied streams.
///
/// EOF, a transport error, or completion of `shutdown` ends the connection
/// and settles its workers. Await this future through shutdown to receive
/// cleanup errors.
pub async fn serve<R, W>(
    config: Config,
    reader: R,
    writer: W,
    shutdown: impl Future<Output = ()>,
) -> Result<(), BoxError>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let (app, mut transport) = connection(config)?;
    let handle = transport.handle();
    run(
        &app.engine,
        transport.run_with_streams(reader, writer),
        handle.stopping(),
        shutdown,
    )
    .await
}

/// Serve one MCP connection on the process's stdin and stdout.
///
/// Use this entry point when the MCP server owns both streams. On Unix, pipe
/// and socket descriptors can temporarily become nonblocking.
pub async fn serve_stdio(
    config: Config,
    shutdown: impl Future<Output = ()>,
) -> Result<(), BoxError> {
    let (app, transport) = connection(config)?;
    let mut transport = transport.pipes(StdioPipes::Shared);
    let handle = transport.handle();
    run(&app.engine, transport.run(), handle.stopping(), shutdown).await
}

fn connection(config: Config) -> Result<(Arc<App>, StdioTransport), BoxError> {
    let app = Arc::new(App {
        engine: Engine::new(config.limits.clone())?,
        config,
    });
    let transport = StdioTransport::without_server_notifications(router(app.clone()))
        .max_concurrent_requests(16)
        .drain_timeout(Duration::from_secs(5));
    Ok((app, transport))
}

async fn run(
    engine: &Engine,
    serving: impl Future<Output = tower_mcp::Result<()>>,
    stopping: impl Future<Output = ()>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), BoxError> {
    let transport = {
        let mut serving = std::pin::pin!(serving);
        tokio::select! {
            biased;
            () = shutdown => Ok(()),
            result = &mut serving => result,
            () = stopping => {
                // EOF can arrive while a tool is still waiting on a worker.
                let (transport, cleanup) = tokio::join!(serving, engine.shutdown());
                return settle(transport, cleanup);
            }
        }
    };
    settle(transport, engine.shutdown().await)
}

fn settle(
    transport: tower_mcp::Result<()>,
    cleanup: Result<(), due_core::Error>,
) -> Result<(), BoxError> {
    match (transport, cleanup) {
        (Err(transport), Err(cleanup)) => {
            Err(format!("{transport}; worker cleanup failed: {cleanup}").into())
        }
        (Err(error), _) => Err(error.into()),
        (_, Err(error)) => Err(error.into()),
        _ => Ok(()),
    }
}
