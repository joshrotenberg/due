//! Newline-delimited JSON-RPC session adapter.

mod stdio;

use std::{future::Future, process::ExitCode, sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinSet,
};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};

use crate::session::{Manager, PushQueue, profile::OpenInput};

const MAX_INPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_REQUESTS: usize = 16;
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(5);

struct Request {
    // Some(null) is a request. Only absence of id makes a notification.
    id: Option<Value>,
    method: String,
    params: Value,
}

#[derive(Debug)]
struct RpcError(i32, String);

impl RpcError {
    fn response(self, id: Value) -> Value {
        json!({"jsonrpc":"2.0", "id":id, "error":{"code":self.0,"message":self.1}})
    }
}

fn parse(line: &str) -> Result<Request, RpcError> {
    let value: Value =
        serde_json::from_str(line).map_err(|_| RpcError(-32700, "Parse error".into()))?;
    let invalid = || {
        RpcError(
            -32600,
            "Invalid Request (one object per line; no batches)".into(),
        )
    };
    let object = value.as_object().ok_or_else(invalid)?;
    if object.get("jsonrpc") != Some(&json!("2.0")) {
        return Err(invalid());
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let id = object.get("id").cloned();
    if id
        .as_ref()
        .is_some_and(|v| !v.is_null() && !v.is_string() && !v.is_i64() && !v.is_u64())
    {
        return Err(invalid());
    }
    // Params shape is checked by dispatch so malformed params on notifications
    // stay silent, while requests receive -32602 with their original ID.
    Ok(Request {
        id,
        method: method.into(),
        params: object.get("params").cloned().unwrap_or_else(|| json!({})),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionInput {
    session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartInput {
    session_id: String,
    prompt: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnInput {
    session_id: String,
    turn_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    session_id: String,
    turn_id: String,
    #[serde(default)]
    after: u64,
    #[serde(default, deserialize_with = "wait_ms")]
    wait_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionReadInput {
    session_id: String,
    #[serde(default)]
    after: u64,
    #[serde(default = "default_limit", deserialize_with = "read_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    64
}

const MAX_WAIT_MS: u64 = 10_000;
const MAX_READ_LIMIT: u64 = 256;

fn wait_ms<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value > MAX_WAIT_MS {
        return Err(serde::de::Error::custom(format!(
            "wait_ms must be at most {MAX_WAIT_MS}"
        )));
    }
    Ok(value)
}

fn read_limit<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if !(1..=MAX_READ_LIMIT).contains(&value) {
        return Err(serde::de::Error::custom(format!(
            "limit must be between 1 and {MAX_READ_LIMIT}"
        )));
    }
    Ok(value as usize)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyInput {
    session_id: String,
    turn_id: String,
    request_id: String,
    response: Value,
}

fn params<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    if !value.is_object() {
        return Err(RpcError(
            -32602,
            "Invalid params: expected named parameters".into(),
        ));
    }
    serde_json::from_value(value)
        .map_err(|error| RpcError(-32602, format!("Invalid params: {error}")))
}

fn describe_open(value: &mut Value) {
    value["protocol_version"] = json!(1);
    value["stream"]["method"] = json!("session.item");
}

fn describe_item(value: &mut Value) {
    if value["kind"] == "session_opened" {
        describe_open(&mut value["data"]);
    }
}

async fn dispatch(manager: &Manager, method: &str, input: Value) -> Result<Value, RpcError> {
    let result = match method {
        "session.open" => {
            let options = params::<OpenInput>(input)?
                .resolve()
                .await
                .map_err(|error| RpcError(-32602, format!("Invalid params: {error}")))?;
            let mut result = manager.open(options).await;
            if let Ok(value) = &mut result {
                describe_open(value);
            }
            result
        }
        "turn.start" => {
            let input: StartInput = params(input)?;
            manager.start(&input.session_id, input.prompt).await
        }
        "turn.read" => {
            let input: ReadInput = params(input)?;
            manager
                .read(
                    &input.session_id,
                    &input.turn_id,
                    input.after,
                    input.wait_ms,
                )
                .await
        }
        "session.read" => {
            let input: SessionReadInput = params(input)?;
            let mut result = manager
                .session_read(&input.session_id, input.after, input.limit)
                .await;
            if let Ok(value) = &mut result
                && let Some(items) = value["items"].as_array_mut()
            {
                for item in items {
                    describe_item(item);
                }
            }
            result
        }
        "turn.interrupt" => {
            let input: TurnInput = params(input)?;
            manager.interrupt(&input.session_id, &input.turn_id).await
        }
        "interaction.reply" => {
            let input: ReplyInput = params(input)?;
            manager
                .reply(
                    &input.session_id,
                    &input.turn_id,
                    &input.request_id,
                    input.response,
                )
                .await
        }
        "session.close" => {
            let input: SessionInput = params(input)?;
            manager.close(Some(&input.session_id)).await
        }
        _ => return Err(RpcError(-32601, "Method not found".into())),
    };
    result.map_err(|message| RpcError(-32000, message))
}

struct Frame {
    line: String,
    // Count queued responses and the currently written response against one
    // byte budget. Drop releases capacity even on a failed connection.
    _bytes: OwnedSemaphorePermit,
}

fn enqueue(
    sender: &mpsc::Sender<Frame>,
    budget: &Arc<Semaphore>,
    value: Value,
) -> Result<(), String> {
    let line = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    if line.len() + 1 > MAX_OUTPUT {
        return Err("RPC response exceeds 8 MiB output limit".into());
    }
    let bytes = budget
        .clone()
        .try_acquire_many_owned((line.len() + 1) as u32)
        .map_err(|_| "RPC response backlog exceeds 8 MiB")?;
    sender
        .try_send(Frame {
            line,
            _bytes: bytes,
        })
        .map_err(|_| "RPC response queue full or closed".into())
}

async fn write_output<W: AsyncWrite + Unpin>(
    writer: W,
    mut responses: mpsc::Receiver<Frame>,
    push: Arc<PushQueue>,
) -> Result<(), String> {
    let mut writer = FramedWrite::new(writer, LinesCodec::new());
    let (mut replies_open, mut items_open) = (true, true);
    while replies_open || items_open {
        // Keep the frame's permit until the complete line is flushed.
        let (line, _held) = tokio::select! {
            frame = responses.recv(), if replies_open => match frame {
                Some(frame) => (frame.line, Some(frame._bytes)),
                None => { replies_open = false; continue; }
            },
            item = push.next(), if items_open => match item {
                Some(mut item) => {
                    describe_item(&mut item);
                    (json!({"jsonrpc":"2.0","method":"session.item","params":item}).to_string(), None)
                },
                None => { items_open = false; continue; }
            },
        };
        // A timed-out partial write ends the connection; no later frame may
        // append to an incomplete JSON message.
        tokio::time::timeout(OUTPUT_TIMEOUT, writer.send(line))
            .await
            .map_err(|_| "RPC stdout stalled for 5 seconds".to_owned())?
            .map_err(|error| format!("RPC stdout: {error}"))?;
    }
    Ok(())
}

async fn run<R, W, F>(reader: R, writer: W, signal: F) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
    F: Future<Output = ()>,
{
    let manager = Arc::new(Manager::default());
    let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_INPUT));
    let (sender, responses) = mpsc::channel(MAX_REQUESTS);
    let budget = Arc::new(Semaphore::new(MAX_OUTPUT));
    let mut output = tokio::spawn(write_output(writer, responses, manager.push.clone()));
    let mut jobs = JoinSet::new();
    let mut output_done = false;
    tokio::pin!(signal);
    let mut outcome = loop {
        tokio::select! {
            () = &mut signal => break Ok(()),
            result = &mut output => {
                output_done = true;
                break result.map_err(|e| format!("RPC output task: {e}")).and_then(|r| r);
            },
            Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                match result {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => break Err(error),
                    Err(error) => break Err(format!("RPC request task: {error}")),
                }
            },
            line = lines.next() => {
                let line = match line {
                    None => break Ok(()),
                    Some(Err(error)) => break Err(format!("RPC stdin: {error}")),
                    Some(Ok(line)) => line,
                };
                let request = match parse(&line) {
                    Ok(request) => request,
                    Err(error) => {
                        if let Err(error) = enqueue(&sender, &budget, error.response(Value::Null)) { break Err(error); }
                        continue;
                    }
                };
                if jobs.len() >= MAX_REQUESTS {
                    if let Some(id) = request.id
                        && let Err(error) = enqueue(&sender, &budget, RpcError(-32001, "Too many in-flight requests".into()).response(id)) {
                        break Err(error);
                    }
                    continue;
                }
                let (manager, sender, budget) = (manager.clone(), sender.clone(), budget.clone());
                jobs.spawn(async move {
                    let result = dispatch(&manager, &request.method, request.params).await;
                    if let Some(id) = request.id {
                        let value = match result {
                            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                            Err(error) => error.response(id),
                        };
                        enqueue(&sender, &budget, value)?;
                    }
                    Ok::<_, String>(())
                });
            },
        }
    };
    // Do not abort an operation while it owns a provider. Shutdown marks the
    // host stopping before acquiring its lock and awaits native settlement.
    if let Err(error) = manager.shutdown().await {
        outcome = outcome.and(Err(error));
    }
    manager.push.close();
    let drained = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(result) = jobs.join_next().await {
            result.map_err(|e| format!("RPC request task: {e}"))??;
        }
        Ok::<_, String>(())
    })
    .await;
    match drained {
        Ok(result) => outcome = outcome.and(result),
        Err(_) => {
            jobs.abort_all();
            while jobs.join_next().await.is_some() {}
            outcome = outcome.and(Err("RPC requests did not settle after shutdown".into()));
        }
    }
    drop(sender);
    if !output_done {
        match tokio::time::timeout(OUTPUT_TIMEOUT, &mut output).await {
            Ok(result) => {
                outcome = outcome.and(
                    result
                        .map_err(|e| format!("RPC output task: {e}"))
                        .and_then(|r| r),
                )
            }
            Err(_) => {
                output.abort();
                let _ = output.await;
                outcome = outcome.and(Err("RPC output drain timed out".into()));
            }
        }
    }
    outcome
}

pub async fn execute() -> ExitCode {
    let signal = match crate::shutdown_signal() {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("due rpc: register shutdown signals: {error}");
            return ExitCode::FAILURE;
        }
    };
    // On Linux pipes can use cancellable async I/O instead of Tokio's blocking
    // stdio fallback. Private descriptors preserve inherited descriptor flags.
    #[cfg(target_os = "linux")]
    {
        let pipes = tokio::net::unix::pipe::OpenOptions::new();
        if let (Ok(reader), Ok(writer)) = (
            pipes.open_receiver("/proc/self/fd/0"),
            pipes.open_sender("/proc/self/fd/1"),
        ) {
            return finish(run(reader, writer, signal).await);
        }
    }
    finish(stdio::execute(signal).await)
}

fn finish(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("due rpc: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        sync::Mutex,
        task::{Context, Poll, Waker},
    };

    #[derive(Default)]
    struct FlushGate {
        released: bool,
        waiting: Option<tokio::sync::oneshot::Sender<()>>,
        waker: Option<Waker>,
    }

    struct GatedWriter(Arc<Mutex<FlushGate>>);

    impl AsyncWrite for GatedWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            let mut gate = self.0.lock().unwrap();
            if gate.released {
                return Poll::Ready(Ok(()));
            }
            gate.waker = Some(cx.waker().clone());
            if let Some(waiting) = gate.waiting.take() {
                let _ = waiting.send(());
            }
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.poll_flush(cx)
        }
    }

    #[test]
    fn absent_id_and_explicit_null_are_different() {
        assert!(
            parse(r#"{"jsonrpc":"2.0","method":"session.close"}"#)
                .unwrap()
                .id
                .is_none()
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","method":"session.close","id":null}"#)
                .unwrap()
                .id,
            Some(Value::Null)
        );
    }

    #[test]
    fn malformed_envelopes_and_batch_have_protocol_errors() {
        assert_eq!(parse("{").err().unwrap().0, -32700);
        for value in [
            "null",
            "[]",
            r#"{"jsonrpc":"1.0","method":"x"}"#,
            r#"{"jsonrpc":"2.0","method":"x","id":{}}"#,
            r#"{"jsonrpc":"2.0","method":"x","id":1.5}"#,
        ] {
            assert_eq!(parse(value).err().unwrap().0, -32600);
        }
    }

    #[tokio::test]
    async fn named_params_are_required_and_unknown_methods_are_errors() {
        let manager = Manager::default();
        assert_eq!(
            dispatch(&manager, "missing", json!({}))
                .await
                .unwrap_err()
                .0,
            -32601
        );
        assert_eq!(
            dispatch(&manager, "session.close", json!([]))
                .await
                .unwrap_err()
                .0,
            -32602
        );
        assert_eq!(
            dispatch(
                &manager,
                "session.close",
                json!({"session_id":"s","typo":1})
            )
            .await
            .unwrap_err()
            .0,
            -32602
        );
    }

    #[tokio::test]
    async fn response_budget_is_released_on_drop() {
        let (tx, mut rx) = mpsc::channel(2);
        let budget = Arc::new(Semaphore::new(32));
        enqueue(&tx, &budget, json!({"test":1})).unwrap();
        let frame = rx.recv().await.unwrap();
        assert!(budget.available_permits() < 32);
        drop(frame);
        assert_eq!(budget.available_permits(), 32);
        assert!(enqueue(&tx, &budget, json!({"too_large":"a".repeat(32)})).is_err());
    }

    #[tokio::test]
    async fn eight_mib_response_budget_includes_a_frame_until_flush_finishes() {
        let (tx, rx) = mpsc::channel(MAX_REQUESTS);
        let budget = Arc::new(Semaphore::new(MAX_OUTPUT));
        // Two JSON quotes and the terminating newline count toward the limit.
        assert_eq!(
            enqueue(&tx, &budget, json!("x".repeat(MAX_OUTPUT - 2))).unwrap_err(),
            "RPC response exceeds 8 MiB output limit"
        );
        assert_eq!(budget.available_permits(), MAX_OUTPUT);
        enqueue(&tx, &budget, json!("x".repeat(MAX_OUTPUT - 3))).unwrap();
        assert_eq!(budget.available_permits(), 0);

        let (waiting, blocked) = tokio::sync::oneshot::channel();
        let gate = Arc::new(Mutex::new(FlushGate {
            waiting: Some(waiting),
            ..FlushGate::default()
        }));
        let push = Arc::new(PushQueue::default());
        push.close();
        let output = tokio::spawn(write_output(GatedWriter(gate.clone()), rx, push));
        tokio::time::timeout(Duration::from_secs(2), blocked)
            .await
            .expect("writer did not reach its flush")
            .unwrap();
        assert_eq!(
            tx.capacity(),
            MAX_REQUESTS,
            "frame must have left the queue"
        );
        assert_eq!(budget.available_permits(), 0, "flush still owns the bytes");
        assert_eq!(
            enqueue(&tx, &budget, Value::Null).unwrap_err(),
            "RPC response backlog exceeds 8 MiB"
        );

        let waker = {
            let mut gate = gate.lock().unwrap();
            gate.released = true;
            gate.waker.take().unwrap()
        };
        waker.wake();
        drop(tx);
        tokio::time::timeout(Duration::from_secs(2), output)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(budget.available_permits(), MAX_OUTPUT);
    }

    #[tokio::test]
    async fn full_response_queue_rejects_without_leaking_byte_permits() {
        let (tx, rx) = mpsc::channel(MAX_REQUESTS);
        let budget = Arc::new(Semaphore::new(MAX_OUTPUT));
        for _ in 0..MAX_REQUESTS {
            enqueue(&tx, &budget, Value::Null).unwrap();
        }
        let remaining = budget.available_permits();
        assert_eq!(
            enqueue(&tx, &budget, Value::Null).unwrap_err(),
            "RPC response queue full or closed"
        );
        assert_eq!(budget.available_permits(), remaining);
        drop(rx);
        assert_eq!(budget.available_permits(), MAX_OUTPUT);
    }

    #[tokio::test]
    async fn immediate_error_response_overload_fails_the_connection() {
        // This test uses the current-thread runtime and an immediately ready
        // reader. Parse errors fill the queue in one poll before the spawned
        // output task can run, independent of kernel pipe buffer sizes.
        let input = "{\n".repeat(MAX_REQUESTS + 1);
        let result = run(input.as_bytes(), tokio::io::sink(), std::future::pending()).await;
        assert_eq!(result.unwrap_err(), "RPC response queue full or closed");
    }
}
