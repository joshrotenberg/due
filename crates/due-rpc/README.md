# Due RPC spike

`due-rpc` runs one `due-core` engine behind a local, newline-delimited
[JSON-RPC 2.0](https://www.jsonrpc.org/specification) connection. The engine
owns all workers for that process. Workers start in the server's working
directory with the installed provider CLI and its native configuration.
The host can set `DUE_RPC_CLAUDE_BINARY` or `DUE_RPC_CODEX_BINARY` before launch.

```sh
cargo build --locked -p due-rpc
cargo test --locked -p due-rpc --test stdio
```

Launch `target/debug/due-rpc` with stdin and stdout piped. This experimental
interface has its own version and method names. It accepts one request object
with an ID and named parameters per line. Read stdout continuously and
correlate responses by ID. Notifications and responses can interleave.

| Method | Parameters and result |
| --- | --- |
| `server.info` | Returns `protocol_version` (currently 1). |
| `run.spawn` | `provider` (`claude` or `codex`), `prompt`, optional `timeout_ms`. Returns a snapshot with a run and live session reference. |
| `session.follow_up` | `session`, `prompt`, optional `timeout_ms`. Returns the next run snapshot in that session. |
| `run.inspect` | `run`. Returns a snapshot with pending interactions or a terminal result. |
| `run.wait` | `runs`, optional `any` and `timeout_ms`. Observes completion for up to ten seconds. Defaults to all runs and one second. Zero polls. |
| `run.interrupt` | `run`. Returns the settled snapshot after interruption. |
| `interaction.respond` | `interaction`, `response`. Submits a provider-native object to an exact pending request. The receipt confirms wrapper acceptance for delivery. |
| `session.read` | `session`, optional `after` (default 0) and `limit` (default 32, range 1–256). Reads a nonblocking page of full items. |
| `session.subscribe` | `session`, optional `after` (default 0). Acknowledges with a `subscription` ID, then sends retained and live items. |
| `session.unsubscribe` | `subscription`. Stops that reader. Already queued items precede the reply. The worker continues running. |

A subscription produces `session.item` notifications with
`params = {subscription, item}`. The item carries its session, optional run,
cursor, kind, full retained payload, and an interaction token when applicable.
Send the token from an interaction item to `interaction.respond` after deciding
how to handle the request. Claude command approval can use
`{"behavior":"allow"}` or `{"behavior":"deny","message":"reason"}`. A Codex
command approval can use `{"decision":"accept"}` or `{"decision":"decline"}`.

Save the last processed item cursor. Pass it as `after` to read or subscribe
again in the same server process. Subscriptions continue across follow-ups.
A page reports `cursor`, `latest_cursor`, and `closed`. A live reader receives
`session.stream_end` with its last cursor and an optional error when the
journal drains after closure or the reader falls behind. A retention gap
contains `kind: "lagged"` and `oldest_available`. Read errors expose this
under `error.data`. Resume from `oldest_available - 1` to acknowledge the gap.

The stdio test client runs both providers, replies to approvals while a wait
is pending, checks cursor replay against a fresh subscription, and receives
live follow-up items. The provider executables in that test are fixtures.
Closing stdin stops admission and settles workers. Input frames have a
1 MiB limit. Output frames have an 8 MiB limit and a five-second write
deadline. Transport failure starts engine cleanup.
