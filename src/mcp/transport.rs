use std::{process::ExitCode, sync::Arc, time::Duration};

use serde::Deserialize;
use serde_json::Value;
use tower_mcp::{CallToolResult, McpRouter, StdioTransport, ToolBuilder, schemars::JsonSchema};

use crate::session::{Manager, OpenOptions};

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "tower_mcp::schemars")]
#[serde(deny_unknown_fields)]
struct SessionInput {
    session_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "tower_mcp::schemars")]
#[serde(deny_unknown_fields)]
struct StartInput {
    session_id: String,
    prompt: String,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "tower_mcp::schemars")]
#[serde(deny_unknown_fields)]
struct TurnInput {
    session_id: String,
    turn_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "tower_mcp::schemars")]
#[serde(deny_unknown_fields)]
struct ReadInput {
    session_id: String,
    turn_id: String,
    /// Last event cursor already read, or zero for the retained window.
    #[serde(default)]
    after: u64,
    /// Wait for a change, at most 10000ms. Pending interactions return immediately.
    #[serde(default)]
    wait_ms: u64,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "tower_mcp::schemars")]
#[serde(deny_unknown_fields)]
struct ReplyInput {
    session_id: String,
    turn_id: String,
    request_id: String,
    /// Provider-native response to the pending interaction shown by turn_read.
    response: Value,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "tower_mcp::schemars")]
#[serde(deny_unknown_fields)]
struct SessionReadInput {
    session_id: String,
    /// Session item cursor, independent of turn_read's event cursor.
    #[serde(default)]
    after: u64,
    /// Maximum items in this page (1..=256). Pages also have a byte limit.
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    64
}

fn result(value: Result<Value, String>) -> CallToolResult {
    match value {
        Ok(value) => CallToolResult::json(value),
        Err(error) => CallToolResult::error(error),
    }
}

fn router(manager: Arc<Manager>) -> McpRouter {
    let open = Arc::clone(&manager);
    let start = Arc::clone(&manager);
    let read = Arc::clone(&manager);
    let interrupt = Arc::clone(&manager);
    let reply = Arc::clone(&manager);
    let session_read = Arc::clone(&manager);
    McpRouter::new()
        .server_info("due-mcp-spike", env!("CARGO_PKG_VERSION"))
        .tool(ToolBuilder::new("session_open")
            .description("Open or resume one resident Claude/Codex session. Native authentication/settings apply. Close the current session before opening another. Resume takes a native ID, not a Due handle. stream=true opts into Due-specific notifications/due/session_item notifications; clients must handle that extension.")
            .handler(move |input: OpenOptions| { let m = open.clone(); async move { Ok(result(m.open(input).await.map(super::open_result))) } }).build())
        .tool(ToolBuilder::new("turn_start")
            .description("Start one turn and return its ID immediately. Use turn_read to obtain events, interactions, and the final result. Read the previous result first; only the latest turn is retained.")
            .handler(move |input: StartInput| { let m = start.clone(); async move { Ok(result(m.start(&input.session_id, input.prompt).await)) } }).build())
        .tool(ToolBuilder::new("turn_read")
            .description("Read the latest turn with an optional bounded wait. Events are a lossy bounded native window; result and pending interactions are authoritative. Advance after to the returned cursor. Content can contain prompts, paths and tool payloads.")
            .read_only()
            .handler(move |input: ReadInput| { let m = read.clone(); async move { Ok(result(m.read(&input.session_id, &input.turn_id, input.after, input.wait_ms).await)) } }).build())
        .tool(ToolBuilder::new("session_read")
            .description("Read buffered session items across turns, including native payloads. Use the session cursor (not turn_read cursor); advance after to cursor and repeat while has_more. Retention is bounded; items_lost reports gaps. Available even without push streaming.")
            .read_only()
            .handler(move |input: SessionReadInput| { let m = session_read.clone(); async move { Ok(result(m.session_read(&input.session_id, input.after, input.limit).await.map(super::session_read_result))) } }).build())
        .tool(ToolBuilder::new("turn_interrupt")
            .description("Request interruption of the current turn. Continue turn_read until it finishes. Interruption is not proof that native tools or side effects have stopped; session_close performs provider shutdown.")
            .handler(move |input: TurnInput| { let m = interrupt.clone(); async move { Ok(result(m.interrupt(&input.session_id, &input.turn_id).await)) } }).build())
        .tool(ToolBuilder::new("interaction_reply")
            .description("Reply to a pending provider request from turn_read. Claude accepts behavior=allow with optional updated_input, or behavior=deny with message. Codex accepts its native response object. Only reply within the caller's authorization; no automatic approvals.")
            .handler(move |input: ReplyInput| { let m = reply.clone(); async move { Ok(result(m.reply(&input.session_id, &input.turn_id, &input.request_id, input.response).await)) } }).build())
        .tool(ToolBuilder::new("session_close")
            .description("Close the session and await native process cleanup, including any active turn. Persistent native session IDs can be resumed in a later open; ephemeral sessions cannot.")
            .handler(move |input: SessionInput| { let m = manager.clone(); async move { Ok(result(m.close(Some(&input.session_id)).await)) } }).build())
}

pub async fn execute() -> ExitCode {
    let manager = Arc::new(Manager::default());
    let push = manager.push.clone();
    let mut transport = StdioTransport::new(router(manager.clone()))
        .max_concurrent_requests(16)
        .max_frame_len(1024 * 1024)
        .drain_timeout(Duration::from_secs(2));
    let handle = transport.handle();
    let (finished, transport_finished) = tokio::sync::oneshot::channel::<()>();
    let signal = match crate::shutdown_signal() {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("due mcp: register shutdown signals: {error}");
            return ExitCode::FAILURE;
        }
    };
    let cleanup_push = push.clone();
    let cleanup = tokio::spawn(async move {
        tokio::select! {
            () = handle.stopping() => {},
            () = signal => { let _ = handle.shutdown(); },
            _ = transport_finished => {},
        }
        let result = manager.shutdown().await;
        cleanup_push.close();
        result
    });
    let outcome = super::push_output::run(&mut transport, push, finished).await;
    let cleanup = cleanup.await;
    if let Err(error) = outcome {
        eprintln!("due mcp: {error}");
        return ExitCode::FAILURE;
    }
    match cleanup {
        Ok(Ok(_)) => ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("due mcp: shutdown: {error}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("due mcp: shutdown task: {error}");
            ExitCode::FAILURE
        }
    }
}
