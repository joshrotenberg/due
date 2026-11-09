use std::{path::PathBuf, time::Duration};

use due_core::{
    Assignment, Engine, FollowUp, InteractionRef, Provider, RunRef, SessionRef, StreamError,
    WaitOptions, WaitUntil, WorkerOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub struct App {
    pub engine: Engine,
    pub cwd: PathBuf,
    pub claude_binary: Option<PathBuf>,
    pub codex_binary: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(
    tag = "op",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Request {
    Spawn {
        provider: Provider,
        prompt: String,
        timeout_ms: Option<u64>,
    },
    FollowUp {
        session: SessionRef,
        prompt: String,
        timeout_ms: Option<u64>,
    },
    Inspect {
        run: RunRef,
    },
    Wait {
        runs: Vec<RunRef>,
        timeout_ms: u64,
    },
    Interrupt {
        run: RunRef,
    },
    Respond {
        interaction: InteractionRef,
        response: serde_json::Map<String, Value>,
    },
    ReadItems {
        session: SessionRef,
        after: u64,
        limit: usize,
    },
}

#[derive(Serialize)]
pub struct Failure {
    kind: Value,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Box<Value>>,
}

impl Failure {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: json!(kind),
            message: message.into(),
            data: None,
        }
    }
}

impl From<due_core::Error> for Failure {
    fn from(error: due_core::Error) -> Self {
        Self {
            kind: json!(error.kind),
            message: error.message,
            data: None,
        }
    }
}

impl From<StreamError> for Failure {
    fn from(error: StreamError) -> Self {
        let data = json!(error);
        Self {
            kind: data["kind"].clone(),
            message: error.to_string(),
            data: Some(Box::new(data)),
        }
    }
}

fn value(value: impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(|error| Failure::new("serialize", error.to_string()))
}

pub async fn dispatch(app: &App, request: Request) -> Result<Value, Failure> {
    match request {
        Request::Spawn {
            provider,
            prompt,
            timeout_ms,
        } => {
            let mut worker = WorkerOptions::new(provider, &app.cwd);
            worker.binary = match provider {
                Provider::Claude => app.claude_binary.clone(),
                Provider::Codex => app.codex_binary.clone(),
            };
            let run = app
                .engine
                .spawn(Assignment {
                    worker,
                    prompt,
                    timeout: timeout_ms.map(Duration::from_millis),
                })
                .await?;
            value(app.engine.inspect(&run)?)
        }
        Request::FollowUp {
            session,
            prompt,
            timeout_ms,
        } => {
            let run = app
                .engine
                .follow_up(
                    session,
                    FollowUp {
                        prompt,
                        timeout: timeout_ms.map(Duration::from_millis),
                    },
                )
                .await?;
            value(app.engine.inspect(&run)?)
        }
        Request::Inspect { run } => value(app.engine.inspect(&run)?),
        Request::Wait { runs, timeout_ms } => {
            if timeout_ms > 10_000 {
                return Err(Failure::new(
                    "invalid_input",
                    "wait timeout must be at most 10000 ms",
                ));
            }
            value(
                app.engine
                    .wait(
                        &runs,
                        WaitOptions {
                            until: WaitUntil::All,
                            timeout: Some(Duration::from_millis(timeout_ms)),
                        },
                    )
                    .await?,
            )
        }
        Request::Interrupt { run } => value(app.engine.interrupt(&run).await?),
        Request::Respond {
            interaction,
            response,
        } => value(
            app.engine
                .respond(&interaction, Value::Object(response))
                .await?,
        ),
        Request::ReadItems {
            session,
            after,
            limit,
        } => {
            let mut reader = app.engine.subscribe(&session, after)?;
            value(reader.read_available(limit)?)
        }
    }
}
