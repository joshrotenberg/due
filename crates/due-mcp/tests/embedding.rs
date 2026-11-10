use std::time::Duration;

use due_mcp::{Config, serve};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex, split},
    sync::oneshot,
    time::timeout,
};

#[tokio::test]
async fn rust_host_supplies_streams_and_shutdown() {
    timeout(Duration::from_secs(5), async {
        let (server, client) = duplex(16 * 1024);
        let (server_read, server_write) = split(server);
        let (client_read, mut client_write) = split(client);
        let mut responses = BufReader::new(client_read).lines();
        let (stop, shutdown) = oneshot::channel();
        let config = Config::new(std::env::current_dir().unwrap());
        let server = tokio::spawn(serve(config, server_read, server_write, async {
            let _ = shutdown.await;
        }));

        let initialize = json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"rust-host","version":"0.1"}
            }
        });
        client_write
            .write_all(format!("{initialize}\n").as_bytes())
            .await
            .unwrap();
        let response: Value =
            serde_json::from_str(&responses.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["serverInfo"]["name"], "due-mcp");

        // Keep the client's write end open so only the host signal stops the server.
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
        assert!(responses.next_line().await.unwrap().is_none());
    })
    .await
    .expect("embedded MCP exchange timed out");
}
