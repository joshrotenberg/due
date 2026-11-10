# Due MCP adapter

`due-mcp` hosts one `due-core` engine per MCP connection. Use `due_mcp::serve`
with your own asynchronous reader and writer. The host supplies a workspace,
an optional executable override for each provider, engine limits, and a
shutdown future. Each call owns its workers and returns after cleanup on EOF,
transport failure, or host shutdown.

```rust
use due_mcp::{Config, serve};
use tokio::io::{AsyncRead, AsyncWrite};

async fn host_connection(
    reader: impl AsyncRead + Send + Unpin,
    writer: impl AsyncWrite + Send + Unpin,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::new("/path/to/authorized/workspace");
    serve(config, reader, writer, std::future::pending()).await
}
```

The Due executable uses `serve_stdio` for its standard streams. Build it with
`cargo build --locked -p due-cli --features mcp-stdio` and configure your MCP
client to launch `due mcp --cwd /path/to/authorized/workspace`. Without
`--cwd`, the server uses its working directory. The CLI accepts the executable
overrides `DUE_MCP_CLAUDE_BINARY` and `DUE_MCP_CODEX_BINARY` from its environment.

Read the [MCP guide](../../embedded-skills/mcp/SKILL.md) for tools, run references,
follow-ups, and approvals. The same guide is available in the feature-enabled
binary with `due skills show mcp`.

Hosts can parse a version-1 JSON catalog with
`ProfileCatalog::from_json` and assign it to `Config::profiles`. The adapter
validates each worker against the connection configuration before opening the
transport. The CLI reads the catalog from a regular JSON file supplied with
`due mcp --profiles FILE`. See the [catalog example](../../embedded-skills/mcp/SKILL.md#named-worker-profiles).
