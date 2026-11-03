use super::*;
use std::{io::Read, os::fd::OwnedFd, os::unix::net::UnixStream};

#[test]
fn malformed_json_is_recoverable_during_an_active_turn() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        client.raw(b"{\n");
        let invalid = client.response(&Value::Null, Duration::from_secs(3));
        assert_eq!(invalid["error"]["code"], -32700);
        assert_eq!(
            client.ok("turn.read", json!({"session_id":session,"turn_id":turn}))["state"],
            "running"
        );
        client.close(&session);
        client.eof();
    }
}

#[test]
fn invalid_utf8_closes_connection_and_reaps_active_provider() {
    for provider in ["claude", "codex"] {
        let mut client = Client::new(false);
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        client.raw(b"\xff\n");
        // Keep stdin open: invalid framing itself must trigger shutdown.
        client.wait_exit(false);
        assert!(client.stdin.is_some());
        assert!(
            client.dir.path().join("exited").exists(),
            "native {provider} fixture survived invalid UTF-8"
        );
        assert_eq!(client.finished(&session, &turn)["status"], "cancelled");
        let pid: i32 = fs::read_to_string(client.dir.path().join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "provider was not reaped");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

#[test]
fn broken_stdout_settles_native_before_a_full_stderr_is_drained() {
    for provider in ["claude", "codex"] {
        let (mut error_reader, mut error_writer) = UnixStream::pair().unwrap();
        error_writer.set_nonblocking(true).unwrap();
        loop {
            match error_writer.write(&[b'x'; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("fill stderr socket: {error}"),
            }
        }
        error_writer.set_nonblocking(false).unwrap();
        let mut client = Client::with_stderr(false, |_| Stdio::from(OwnedFd::from(error_writer)));
        let session = client.open(provider, true, None);
        let turn = client.start(&session, "hold");
        client.ready(provider, &session, &turn);
        client.close_stdout.store(true, Ordering::Release);
        client.request("missing", json!({}));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match client
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => panic!("stdout reader did not close: {error}"),
            }
        }
        client.request("missing", json!({}));
        let deadline = Instant::now() + Duration::from_secs(3);
        let exited = client.dir.path().join("exited");
        while !exited.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let settled_before_drain = exited.exists();
        // Drain only after observing native cleanup, or the guard expiring.
        // Final diagnostics may block Due's exit; they must not block cleanup.
        let drain =
            thread::spawn(move || std::io::copy(&mut error_reader, &mut std::io::sink()).unwrap());
        client.wait_exit(false);
        drain.join().unwrap();
        assert!(
            settled_before_drain,
            "{provider}: stderr diagnostics blocked native cleanup"
        );
    }
}

#[test]
fn concurrent_request_limit_is_recoverable_when_output_is_draining() {
    let mut client = Client::new(false);
    let session = client.open("codex", true, None);
    client
        .pushed_until(|item| item["kind"] == "native" && item["data"]["method"] == "fixture/idle");
    let turn = client.start(&session, "startup-hold");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !client.dir.path().join("start-waiting").exists() {
        assert!(
            Instant::now() < deadline,
            "native startup did not reach its gate"
        );
        thread::sleep(Duration::from_millis(10));
    }
    // The fixture emits no more events until release-start is created. Each
    // read therefore holds an admission slot instead of racing native output.
    let snapshot = client.ok("turn.read", json!({"session_id":session,"turn_id":turn}));
    let reads: Vec<_> = (0..16)
        .map(|_| {
            client.request(
                "turn.read",
                json!({"session_id":session,"turn_id":turn,"after":snapshot["cursor"],"wait_ms":10000}),
            )
        })
        .collect();
    let rejected = client.request("missing", json!({}));
    assert_eq!(
        client.response(&rejected, Duration::from_secs(3))["error"]["code"],
        -32001
    );
    fs::write(client.dir.path().join("release-start"), "").unwrap();
    for id in reads {
        let response = client.response(&id, Duration::from_secs(3));
        assert!(response.get("error").is_none(), "{response}");
    }
    assert_eq!(client.call("missing", json!({}))["error"]["code"], -32601);
    client.close(&session);
    client.eof();
}

struct OwnedRpcChild(Child);

impl Drop for OwnedRpcChild {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn wait_child(child: &mut OwnedRpcChild, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "RPC process did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn aggregate_response_byte_overflow_fails_with_both_pipes_open() {
    let mut child = OwnedRpcChild(
        Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let _output = child.0.stdout.take().unwrap();
    let mut stderr = child.0.stderr.take().unwrap();
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        // Each request fits the 1 MiB input limit. Ten echoed IDs exceed the
        // aggregate response byte limit before the 16-frame queue limit.
        let result = (0..10).try_for_each(|id| {
            let request = json!({
                "jsonrpc":"2.0", "method":"missing",
                "id":format!("{id}{}", "x".repeat(900 * 1024)),
            });
            writeln!(input, "{request}")
        });
        let _ = sent.send((input, result));
    });
    // Holding both endpoints open distinguishes backlog overflow from EOF or
    // a broken pipe. Cleanup still has to bound the stalled final drain.
    let (_input, result) = received
        .recv_timeout(Duration::from_secs(6))
        .expect("Due did not consume the bounded input burst");
    result.unwrap();
    assert!(!wait_child(&mut child, Duration::from_secs(12)).success());
    let mut error = String::new();
    stderr.read_to_string(&mut error).unwrap();
    assert!(
        error.contains("RPC response backlog exceeds 8 MiB"),
        "{error}"
    );
}

#[test]
fn unix_socket_stdio_round_trip_and_signal_shutdown_with_input_open() {
    // Node's spawn pipes can be Unix sockets. On Linux these cannot be opened
    // as /proc pipe descriptors, so this explicitly exercises the fallback;
    // the Stdio::piped cases cover the Linux async pipe path separately.
    let (mut input, child_input) = UnixStream::pair().unwrap();
    let (output, child_output) = UnixStream::pair().unwrap();
    output
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut output = BufReader::new(output);
    let mut child = OwnedRpcChild(
        Command::new(env!("CARGO_BIN_EXE_due"))
            .arg("rpc")
            .stdin(Stdio::from(OwnedFd::from(child_input)))
            .stdout(Stdio::from(OwnedFd::from(child_output)))
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"missing"})
    )
    .unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["id"], 1);
    assert_eq!(response["error"]["code"], -32601);
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    assert!(wait_child(&mut child, Duration::from_secs(3)).success());
    drop(input);
}
