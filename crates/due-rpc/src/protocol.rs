//! Wire shapes for the standalone adapter.

use std::{path::PathBuf, time::Duration};

use due_core::{
    Assignment, Engine, FollowUp, InteractionRef, Provider, RunRef, SessionRef, StreamError,
    WaitOptions, WaitUntil, WorkerOptions,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub struct App {
    pub engine: Engine,
    pub cwd: PathBuf,
    pub claude_binary: Option<PathBuf>,
    pub codex_binary: Option<PathBuf>,
}

pub struct Request {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

#[derive(Serialize)]
pub struct RpcError {
    pub code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn with_data(code: i32, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }
}

impl From<due_core::Error> for RpcError {
    fn from(error: due_core::Error) -> Self {
        let code = match error.kind {
            due_core::ErrorKind::InvalidInput => -32602,
            _ => -32000,
        };
        Self::with_data(code, error.message, json!({"kind": error.kind}))
    }
}

impl From<StreamError> for RpcError {
    fn from(error: StreamError) -> Self {
        let code = match error {
            StreamError::Lagged { .. } => -32002,
            _ => -32602,
        };
        Self::with_data(code, error.to_string(), json!(error))
    }
}

pub fn parse(line: &str) -> Result<Request, RpcError> {
    let value: Value =
        serde_json::from_str(line).map_err(|_| RpcError::new(-32700, "Parse error"))?;
    let invalid = || RpcError::new(-32600, "Invalid Request");
    let object = value.as_object().ok_or_else(invalid)?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(invalid());
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let id = object.get("id").cloned();
    if id
        .as_ref()
        .is_some_and(|id| !id.is_null() && !id.is_string() && !id.is_number())
    {
        return Err(invalid());
    }
    Ok(Request {
        id,
        method: method.into(),
        params: object.get("params").cloned().unwrap_or_else(|| json!({})),
    })
}

pub fn params<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    if !value.is_object() {
        return Err(RpcError::new(-32602, "Expected named parameters"));
    }
    serde_json::from_value(value).map_err(|error| RpcError::new(-32602, error.to_string()))
}

pub fn reply(id: Value, result: Result<Value, RpcError>) -> Value {
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error}),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spawn {
    provider: Provider,
    prompt: String,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Follow {
    session: SessionRef,
    prompt: String,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    run: RunRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wait {
    runs: Vec<RunRef>,
    #[serde(default)]
    any: bool,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Respond {
    interaction: InteractionRef,
    response: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    session: SessionRef,
    #[serde(default)]
    after: u64,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn value(value: impl Serialize) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|error| RpcError::new(-32603, error.to_string()))
}

pub async fn dispatch(app: &App, method: &str, input: Value) -> Result<Value, RpcError> {
    match method {
        "server.info" => {
            let _: Empty = params(input)?;
            Ok(json!({"name":"due-rpc","protocol_version":1}))
        }
        "run.spawn" => {
            let input: Spawn = params(input)?;
            let mut worker = WorkerOptions::new(input.provider, &app.cwd);
            worker.binary = match input.provider {
                Provider::Claude => app.claude_binary.clone(),
                Provider::Codex => app.codex_binary.clone(),
            };
            let run = app
                .engine
                .spawn(Assignment {
                    worker,
                    prompt: input.prompt,
                    timeout: input.timeout_ms.map(Duration::from_millis),
                })
                .await?;
            value(app.engine.inspect(&run)?)
        }
        "session.follow_up" => {
            let input: Follow = params(input)?;
            let run = app
                .engine
                .follow_up(
                    input.session,
                    FollowUp {
                        prompt: input.prompt,
                        timeout: input.timeout_ms.map(Duration::from_millis),
                    },
                )
                .await?;
            value(app.engine.inspect(&run)?)
        }
        "run.inspect" => {
            let input: Run = params(input)?;
            value(app.engine.inspect(&input.run)?)
        }
        "run.wait" => {
            let input: Wait = params(input)?;
            let ms = input.timeout_ms.unwrap_or(1000);
            if ms > 10_000 {
                return Err(RpcError::new(-32602, "timeout_ms must be at most 10000"));
            }
            let until = if input.any {
                WaitUntil::Any
            } else {
                WaitUntil::All
            };
            value(
                app.engine
                    .wait(
                        &input.runs,
                        WaitOptions {
                            until,
                            timeout: Some(Duration::from_millis(ms)),
                        },
                    )
                    .await?,
            )
        }
        "run.interrupt" => {
            let input: Run = params(input)?;
            value(app.engine.interrupt(&input.run).await?)
        }
        "interaction.respond" => {
            let input: Respond = params(input)?;
            value(
                app.engine
                    .respond(&input.interaction, Value::Object(input.response))
                    .await?,
            )
        }
        "session.read" => {
            let input: Read = params(input)?;
            let mut stream = app.engine.subscribe(&input.session, input.after)?;
            value(stream.read_available(input.limit.unwrap_or(32))?)
        }
        _ => Err(RpcError::new(-32601, "Method not found")),
    }
}
