use std::{future::Future, io, sync::Arc, time::Duration};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tower_mcp::StdioTransport;

use crate::session::PushQueue;

const PIPE_BYTES: usize = 64 * 1024;
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

// Keep native stream notifications outside tower-mcp's logging channel: they
// are opt-in session data, and logging filters must not change their delivery.
pub(super) async fn run(
    transport: &mut StdioTransport,
    queue: Arc<PushQueue>,
    finished: tokio::sync::oneshot::Sender<()>,
) -> Result<(), String> {
    // Match StdioPipes::Auto on Linux. Opening /proc paths creates private
    // descriptions, so Tokio's O_NONBLOCK does not affect inherited handles.
    // Tokio validates both opened descriptors are pipes. Other stream types
    // and platforms retain tower-mcp's default blocking-handle fallback.
    #[cfg(target_os = "linux")]
    {
        let pipes = tokio::net::unix::pipe::OpenOptions::new();
        if let (Ok(reader), Ok(writer)) = (
            pipes.open_receiver("/proc/self/fd/0"),
            pipes.open_sender("/proc/self/fd/1"),
        ) {
            return run_with_streams(transport, reader, writer, queue, finished).await;
        }
    }
    run_with_streams(
        transport,
        tokio::io::stdin(),
        tokio::io::stdout(),
        queue,
        finished,
    )
    .await
}

async fn run_with_streams<R, W>(
    transport: &mut StdioTransport,
    reader: R,
    writer: W,
    queue: Arc<PushQueue>,
    finished: tokio::sync::oneshot::Sender<()>,
) -> Result<(), String>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let (rpc_reader, rpc_writer) = tokio::io::duplex(PIPE_BYTES);
    let handle = transport.handle();
    let protocol = transport.run_with_streams(reader, rpc_writer);
    let output = multiplex(rpc_reader, writer, queue.as_ref());
    tokio::pin!(protocol, output);

    tokio::select! {
        result = &mut protocol => {
            // Cleanup may emit terminal items. It closes the queue after
            // provider shutdown; let the caller receive those during drain.
            let _ = finished.send(());
            let written = tokio::time::timeout(OUTPUT_TIMEOUT, &mut output).await;
            result.map_err(|error| error.to_string())?;
            written
                .map_err(|_| "session output drain timed out".to_owned())?
                .map_err(|error| format!("session output: {error}"))
        }
        result = &mut output => {
            // multiplex owns the duplex reader: returning drops it and
            // releases a protocol write stalled behind a failed stdout.
            queue.close();
            // Errors can bypass tower-mcp's normal stopping notification.
            let _ = finished.send(());
            let _ = handle.shutdown();
            let stopped = tokio::time::timeout(STOP_TIMEOUT, &mut protocol).await;
            result.map_err(|error| format!("session output: {error}"))?;
            stopped
                .map_err(|_| "session transport shutdown timed out".to_owned())?
                .map_err(|error| error.to_string())
        }
    }
}

trait Notifications {
    fn next(&self) -> impl Future<Output = Option<Value>> + Send;
}

impl Notifications for PushQueue {
    async fn next(&self) -> Option<Value> {
        PushQueue::next(self).await.map(|item| {
            serde_json::json!({"jsonrpc":"2.0","method":super::ITEM_METHOD,"params":super::session_item(item)})
        })
    }
}

async fn multiplex<R, W, Q>(reader: R, mut writer: W, queue: &Q) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    Q: Notifications,
{
    // next_line retains a partial RPC frame when the notification branch wins
    // select. A fresh read_line future would lose those consumed bytes.
    let mut lines = BufReader::new(reader).lines();
    let mut rpc_open = true;
    let mut notifications_open = true;
    while rpc_open || notifications_open {
        let frame = tokio::select! {
            line = lines.next_line(), if rpc_open => match line? {
                Some(line) => line,
                None => { rpc_open = false; continue; }
            },
            value = queue.next(), if notifications_open => match value {
                Some(value) => serde_json::to_string(&value)?,
                None => { notifications_open = false; continue; }
            },
        };
        // Only this task writes stdout. A deadline ends the connection; a
        // partially written frame is never resumed by a different message.
        tokio::time::timeout(OUTPUT_TIMEOUT, async {
            writer.write_all(frame.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "session output stalled"))??;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use serde_json::json;
    use tokio::sync::{Mutex, mpsc};

    use super::*;

    struct TestQueue(Mutex<mpsc::Receiver<Value>>);

    impl Notifications for TestQueue {
        async fn next(&self) -> Option<Value> {
            self.0.lock().await.recv().await
        }
    }

    #[tokio::test]
    async fn partial_rpc_frames_survive_interleaved_notifications() {
        let (reader, mut rpc_writer) = tokio::io::duplex(8);
        let (send, receive) = mpsc::channel(1);
        let queue = TestQueue(Mutex::new(receive));
        let notice = json!({"jsonrpc":"2.0", "method":"notifications/due/session_item", "params":{"cursor":1}});
        let expected_notice = notice.clone();
        let producer = tokio::spawn(async move {
            rpc_writer
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":")
                .await
                .unwrap();
            send.send(notice).await.unwrap();
            // Let the pending next_line lose a select race after reading the
            // prefix, before the remainder of the RPC frame exists.
            tokio::task::yield_now().await;
            rpc_writer.write_all(b"1,\"result\":{}}\n").await.unwrap();
        });
        let mut output = Vec::new();
        multiplex(reader, &mut output, &queue).await.unwrap();
        producer.await.unwrap();
        let frames: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames.len(), 2);
        assert!(frames.contains(&expected_notice));
        assert!(frames.contains(&json!({"jsonrpc":"2.0", "id":1, "result":{}})));
    }

    #[tokio::test]
    async fn input_eof_drains_terminal_items_emitted_by_cleanup() {
        let queue = Arc::new(PushQueue::default());
        let cleanup_queue = queue.clone();
        let (finished, transport_finished) = tokio::sync::oneshot::channel();
        let cleanup = tokio::spawn(async move {
            transport_finished.await.unwrap();
            tokio::task::yield_now().await;
            let items = crate::session::SessionItems::new("s".into(), Some(cleanup_queue.clone()));
            items.record("turn_finished", json!({"status":"interrupted"}), Some("t"));
            cleanup_queue.close();
        });
        let mut transport = StdioTransport::new(tower_mcp::McpRouter::new());
        let mut output = Vec::new();
        run_with_streams(
            &mut transport,
            tokio::io::empty(),
            &mut output,
            queue,
            finished,
        )
        .await
        .unwrap();
        cleanup.await.unwrap();
        let notification: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(notification["method"], "notifications/due/session_item");
        assert_eq!(notification["params"]["kind"], "turn_finished");
        assert_eq!(notification["params"]["data"]["status"], "interrupted");
    }

    struct BrokenWriter;

    impl AsyncWrite for BrokenWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "closed peer",
            )))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn broken_output_releases_the_protocol_pipe() {
        let (reader, mut rpc_writer) = tokio::io::duplex(8);
        let (send, receive) = mpsc::channel(1);
        let queue = TestQueue(Mutex::new(receive));
        send.send(json!({"jsonrpc":"2.0", "method":"notifications/due/session_item"}))
            .await
            .unwrap();
        let error = multiplex(reader, BrokenWriter, &queue).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        let error = rpc_writer.write_all(b"response\n").await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
