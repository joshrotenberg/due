mod protocol;

use std::{collections::HashMap, io, sync::Arc, time::Duration};

use due_core::{Engine, Limits, SessionRef, SessionStream};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};

use protocol::{App, RpcError, dispatch, params, parse, reply};

const MAX_INPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_REQUESTS: usize = 16;
const MAX_SUBSCRIPTIONS: usize = 16;
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscribe {
    session: SessionRef,
    #[serde(default)]
    after: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Unsubscribe {
    subscription: u64,
}

#[derive(Clone)]
struct Output(mpsc::Sender<String>);

impl Output {
    async fn send(&self, value: Value) -> io::Result<()> {
        // Reserve before serializing so the queue holds at most one encoded frame
        // in addition to the one being written. A stalled writer closes the queue.
        let permit = self.0.reserve().await.map_err(io::Error::other)?;
        let line = serde_json::to_string(&value).map_err(io::Error::other)?;
        if line.len() + 1 > MAX_OUTPUT {
            return Err(io::Error::other("RPC frame exceeds 8 MiB"));
        }
        permit.send(line);
        Ok(())
    }
}

async fn write_output(
    writer: impl AsyncWrite + Unpin,
    mut lines: mpsc::Receiver<String>,
) -> io::Result<()> {
    let mut writer = FramedWrite::new(writer, LinesCodec::new());
    while let Some(line) = lines.recv().await {
        tokio::time::timeout(OUTPUT_TIMEOUT, writer.send(line))
            .await
            .map_err(|_| io::Error::other("RPC stdout stalled"))?
            .map_err(io::Error::other)?;
    }
    Ok(())
}

async fn stream_items(
    mut stream: SessionStream,
    subscription: u64,
    session: SessionRef,
    output: Output,
) {
    loop {
        let error = match stream.next().await {
            Ok(Some(item)) => {
                if output.send(json!({
                    "jsonrpc":"2.0","method":"session.item","params":{"subscription":subscription,"item":item}
                })).await.is_err() {
                    return;
                }
                continue;
            }
            Ok(None) => None,
            Err(error) => Some(error),
        };
        let _ = output.send(json!({
            "jsonrpc":"2.0","method":"session.stream_end",
            "params":{"subscription":subscription,"session":session,"cursor":stream.cursor(),"error":error}
        })).await;
        return;
    }
}

#[derive(Default)]
struct Subscriptions {
    next: u64,
    active: HashMap<u64, JoinHandle<()>>,
}

impl Subscriptions {
    async fn subscribe(
        &mut self,
        app: &App,
        id: Value,
        input: Value,
        out: &Output,
    ) -> io::Result<()> {
        let admission = (|| {
            let input: Subscribe = params(input)?;
            self.active.retain(|_, task| !task.is_finished());
            if self.active.len() >= MAX_SUBSCRIPTIONS {
                return Err(RpcError::new(-32001, "Too many subscriptions"));
            }
            let stream = app.engine.subscribe(&input.session, input.after)?;
            Ok((input, stream))
        })();
        let (input, stream) = match admission {
            Ok(pair) => pair,
            Err(error) => return out.send(reply(id, Err(error))).await,
        };
        self.next += 1;
        let subscription = self.next;
        // The acknowledgement enters the same FIFO before any item.
        out.send(reply(
            id,
            Ok(json!({
                "subscription":subscription,"session":input.session,"cursor":input.after
            })),
        ))
        .await?;
        let task = tokio::spawn(stream_items(
            stream,
            subscription,
            input.session,
            out.clone(),
        ));
        self.active.insert(subscription, task);
        Ok(())
    }

    async fn unsubscribe(&mut self, id: Value, input: Value, out: &Output) -> io::Result<()> {
        let result = match params::<Unsubscribe>(input) {
            Ok(input) => {
                let task = self.active.remove(&input.subscription);
                let existed = task.is_some();
                if let Some(task) = task {
                    task.abort();
                    let _ = task.await;
                }
                Ok(json!({"unsubscribed":existed}))
            }
            Err(error) => Err(error),
        };
        out.send(reply(id, result)).await
    }

    async fn stop(self) {
        for task in self.active.values() {
            task.abort();
        }
        for task in self.active.into_values() {
            let _ = task.await;
        }
    }
}

async fn serve(
    app: Arc<App>,
    reader: impl AsyncRead + Unpin,
    writer: impl AsyncWrite + Unpin + Send + 'static,
) -> io::Result<()> {
    let mut input = FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_INPUT));
    let (tx, rx) = mpsc::channel(1);
    let output = Output(tx);
    let mut writer = tokio::spawn(write_output(writer, rx));
    let mut writer_finished = false;
    let mut requests = JoinSet::new();
    let mut subscriptions = Subscriptions::default();

    let result = loop {
        tokio::select! {
            result = &mut writer => {
                writer_finished = true;
                break result.map_err(io::Error::other).and_then(|result| result);
            }
            Some(result) = requests.join_next(), if !requests.is_empty() => {
                if let Err(error) = result.map_err(io::Error::other).and_then(|result| result) {
                    break Err(error);
                }
            }
            line = input.next() => {
                let line = match line {
                    Some(Ok(line)) => line,
                    Some(Err(error)) => break Err(io::Error::other(error)),
                    None => break Ok(()),
                };
                let request = match parse(&line) {
                    Ok(request) => request,
                    Err(error) => {
                        if let Err(error) = output.send(reply(Value::Null, Err(error))).await {
                            break Err(error);
                        }
                        continue;
                    }
                };
                let Some(id) = request.id else { continue };
                let immediate = match request.method.as_str() {
                    "session.subscribe" => subscriptions.subscribe(&app, id, request.params, &output).await,
                    "session.unsubscribe" => subscriptions.unsubscribe(id, request.params, &output).await,
                    _ if requests.len() >= MAX_REQUESTS => {
                        output.send(reply(id, Err(RpcError::new(-32001, "Too many in-flight requests")))).await
                    }
                    _ => {
                        let app = app.clone();
                        let output = output.clone();
                        requests.spawn(async move {
                            let result = dispatch(&app, &request.method, request.params).await;
                            output.send(reply(id, result)).await
                        });
                        Ok(())
                    }
                };
                if let Err(error) = immediate {
                    break Err(error);
                }
            }
        }
    };

    subscriptions.stop().await;
    let cleanup = app.engine.shutdown().await.map_err(io::Error::other);
    let drained = tokio::time::timeout(OUTPUT_TIMEOUT, async {
        while let Some(result) = requests.join_next().await {
            result.map_err(io::Error::other)??;
        }
        Ok(())
    })
    .await;
    let requests_result = match drained {
        Ok(result) => result,
        Err(_) => Err(io::Error::other("RPC requests did not drain")),
    };
    requests.shutdown().await;
    drop(output);
    let written = if writer_finished {
        Ok(())
    } else {
        match tokio::time::timeout(OUTPUT_TIMEOUT, &mut writer).await {
            Ok(result) => result.map_err(io::Error::other).and_then(|result| result),
            Err(_) => {
                writer.abort();
                Err(io::Error::other("RPC stdout did not drain"))
            }
        }
    };
    result.and(cleanup).and(requests_result).and(written)
}

async fn run() -> io::Result<()> {
    let app = Arc::new(App {
        engine: Engine::new(Limits::default()).map_err(io::Error::other)?,
        cwd: std::env::current_dir()?,
        claude_binary: std::env::var_os("DUE_RPC_CLAUDE_BINARY").map(Into::into),
        codex_binary: std::env::var_os("DUE_RPC_CODEX_BINARY").map(Into::into),
    });
    serve(app, tokio::io::stdin(), tokio::io::stdout()).await
}

fn main() -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run());
    // Tokio's OS stdio calls can remain blocked after the connection fails.
    // Engine cleanup has settled before process exit releases those threads.
    runtime.shutdown_background();
    result
}
