# Due MCP spike

`due-mcp` is a local stdio server over `due-core`, using tower-mcp for
the MCP handshake, tool schemas, request routing, and transport. One server
process owns an engine. Each spawned worker starts in the server's working
directory, using the installed provider CLI and its native configuration.
The host can set `DUE_MCP_CLAUDE_BINARY` or `DUE_MCP_CODEX_BINARY` to select
an executable at launch. Closing stdin starts engine shutdown, including
runs waiting for approval.

Build the binary with `cargo build --locked -p due-mcp` and configure an MCP
client to launch `target/debug/due-mcp` over stdio. The integration test
launches the same binary and substitutes fake provider executables:

```sh
cargo test --locked -p due-mcp --test stdio
```

| Tool | Arguments and behavior |
| --- | --- |
| `spawn` | `provider` (`claude` or `codex`), `prompt`, optional `timeout_ms`. Returns a run snapshot with a run and a live session reference. |
| `follow_up` | `session`, `prompt`, optional `timeout_ms`. Starts the next turn in that live session. |
| `inspect` | `run`. Returns the current snapshot with pending interactions or a terminal result. |
| `wait` | `runs`, optional `any` and `timeout_ms`. Waits for all runs by default. The observation budget defaults to one second and is capped at ten seconds. Zero polls. |
| `interrupt` | `run`. Requests interruption and returns the settled snapshot. |
| `respond` | `interaction`, `response`. Pass an exact pending token and a provider-native JSON object. The receipt confirms wrapper acceptance for delivery. |
| `read_items` | `session`, optional `after` (default 0) and `limit` (default 32, range 1–256). Returns a bounded page of currently available full items. |

A caller can spawn both providers, wait for their first runs, then call
`follow_up` with each returned session. If a run is `awaiting_input`, inspect
the native request and pass its `interactions[].id` back to `respond`. The
same token is available as `items[].interaction` on its journal item. Example
responses are `{"behavior":"deny","message":"Read-only task"}` for Claude
and `{"decision":"decline"}` for a Codex command approval. The caller decides
how to handle a request. A wait expiring leaves the run active.

Use the returned `cursor` as the next `after`. A page can be empty while
its session is active. When `cursor` reaches `latest_cursor`, the page is
caught up at that instant. The `closed` flag means the journal is sealed,
so a closed and caught-up journal has no more items. A retention gap is an
MCP tool error with `error.kind = "lagged"` and an `oldest_available` cursor.
Restart at `oldest_available - 1` to explicitly acknowledge the gap.
Pages use the [core journal's size limits](../../docs/core-spike.md#full-session-item-stream).
Results carry structured JSON and a compact JSON text representation.
