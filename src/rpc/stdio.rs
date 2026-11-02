//! Bounded stdio bridges whose blocking OS reads do not hold Tokio shutdown open.

use std::{
    future::Future,
    io::{Read, Write},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{oneshot, watch},
};

const PIPE_BYTES: usize = 64 * 1024;
const OUTPUT_DRAIN: Duration = Duration::from_secs(5);

pub(super) async fn execute(signal: impl Future<Output = ()>) -> Result<(), String> {
    let (mut input_writer, input_reader) = tokio::io::duplex(PIPE_BYTES);
    let (output_writer, mut output_reader) = tokio::io::duplex(PIPE_BYTES);
    let (input_done, mut input_result) = oneshot::channel();
    let (output_done, mut output_result) = oneshot::channel();
    let (writing, writes) = watch::channel(None);
    let output_runtime = tokio::runtime::Handle::current();
    std::thread::Builder::new()
        .name("due-rpc-stdout".into())
        .spawn(move || {
            let mut output = std::io::stdout().lock();
            let result = (|| -> Result<(), String> {
                let mut bytes = [0; 8192];
                loop {
                    let count = output_runtime
                        .block_on(output_reader.read(&mut bytes))
                        .map_err(|error| format!("RPC stdout bridge: {error}"))?;
                    if count == 0 {
                        return Ok(());
                    }
                    writing.send_replace(Some(tokio::time::Instant::now()));
                    let written = output
                        .write_all(&bytes[..count])
                        .and_then(|()| output.flush());
                    writing.send_replace(None);
                    if let Err(error) = &written {
                        // One line at detection time. The final error is only
                        // printed after provider cleanup, so without this a
                        // slow cleanup is indistinguishable from an EPIPE that
                        // was never observed.
                        let _ = writeln!(
                            std::io::stderr(),
                            "due rpc: stdout bridge write failed ({} bytes): {error}",
                            count
                        );
                    }
                    written.map_err(|error| format!("RPC stdout: {error}"))?;
                }
            })();
            let _ = output_done.send(result);
        })
        .map_err(|error| format!("start RPC stdout bridge: {error}"))?;

    let input_runtime = tokio::runtime::Handle::current();
    std::thread::Builder::new()
        .name("due-rpc-stdin".into())
        .spawn(move || {
            let mut input = std::io::stdin().lock();
            let result = (|| -> Result<(), String> {
                let mut bytes = [0; 8192];
                loop {
                    let count = match input.read(&mut bytes) {
                        Ok(0) => return Ok(()),
                        Ok(count) => count,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(format!("RPC stdin: {error}")),
                    };
                    if input_runtime
                        .block_on(input_writer.write_all(&bytes[..count]))
                        .is_err()
                    {
                        // The RPC loop closed its read end during shutdown.
                        return Ok(());
                    }
                }
            })();
            // Publish the read outcome before dropping the pipe and delivering
            // EOF, so a normal EOF path cannot overlook an OS read failure.
            let _ = input_done.send(result);
            drop(input_writer);
        })
        .map_err(|error| format!("start RPC stdin bridge: {error}"))?;

    let mut early_output = None;
    let mut outcome = super::run(input_reader, output_writer, async {
        tokio::select! {
            () = signal => {},
            result = &mut output_result => { early_output = Some(result); },
            error = output_stall(writes) => { early_output = Some(Ok(Err(error))); },
        }
    })
    .await;
    if early_output.is_some() {
        // Pairs with the detection line above: shows provider cleanup finished.
        let _ = writeln!(
            std::io::stderr(),
            "due rpc: shutdown after output bridge stopped finished (ok: {})",
            outcome.is_ok()
        );
    }
    match input_result.try_recv() {
        Ok(result) => outcome = outcome.and(result),
        Err(oneshot::error::TryRecvError::Empty) => {}
        Err(oneshot::error::TryRecvError::Closed) => {
            outcome = outcome.and(Err("RPC stdin bridge stopped without a result".into()));
        }
    }
    let written = match early_output {
        Some(result) => Ok(result),
        None => tokio::time::timeout(OUTPUT_DRAIN, output_result).await,
    };
    outcome.and(match written {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("RPC stdout bridge stopped without a result".into()),
        Err(_) => Err("RPC stdout bridge drain timed out".into()),
    })
    // The stdin thread may remain blocked in read after a signal, and stdout
    // may remain blocked after a failed drain. Neither is a Tokio blocking
    // task, so they cannot delay runtime shutdown. This is an executable-only
    // bridge: process exit ends those detached threads after provider cleanup.
}

async fn output_stall(mut writes: watch::Receiver<Option<tokio::time::Instant>>) -> String {
    loop {
        let started = *writes.borrow_and_update();
        let changed = if let Some(started) = started {
            tokio::select! {
                () = tokio::time::sleep_until(started + OUTPUT_DRAIN) => {
                    return "RPC stdout write/flush stalled for 5 seconds".into();
                },
                changed = writes.changed() => changed,
            }
        } else {
            writes.changed().await
        };
        if changed.is_err() {
            // The completion channel reports the writer's outcome.
            return std::future::pending().await;
        }
    }
}
