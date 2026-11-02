# MCP stdio spike

The [plain JSON-RPC alternative](rpc.md) exposes the same resident session host
through direct methods and `session.item` notifications. It removes MCP
initialization/tool envelopes; both adapters preserve the same provider and
session behavior. Build both with `--features mcp,rpc` to compare them.

Build the opt-in feature and launch it from an MCP client:

```sh
cargo build --locked --features mcp
./target/debug/due mcp
```

Default builds and the `due run` request/result contract are unchanged. This
experiment adds one resident session per MCP process. Claude uses
`DuplexSession`; Codex uses `AppServer`. Authentication, model defaults, native
settings and permission policies belong to the selected provider. No agent loop,
configuration store, daemon, HTTP endpoint or TOML layer is added.

A client launches `due mcp` and completes the standard MCP initialization over
stdio. Stdout contains only MCP messages; stderr is diagnostic. Use MCP
`tools/list` for the installed input schemas. Tool results contain both JSON
`structuredContent` and a text representation. A failed operation has `isError`.

## Seven tools

| Tool | Input | Result |
| --- | --- | --- |
| `session_open` | `provider`, optional `model`, `effort`, `cwd`, `binary`, `resume`, `ephemeral`, `timeout`, `stream` | Due `session_id`, available native ID, capabilities |
| `turn_start` | `session_id`, `prompt` | `turn_id` immediately |
| `turn_read` | `session_id`, `turn_id`, optional `after`, `wait_ms` | State, native events, pending interactions, final result |
| `session_read` | `session_id`, optional `after`, `limit` | Buffered session items across turns, recovery cursor and loss indicators |
| `turn_interrupt` | `session_id`, `turn_id` | Interruption requested; read until terminal |
| `interaction_reply` | `session_id`, `turn_id`, `request_id`, `response` | Reply accepted for delivery |
| `session_close` | `session_id` | Awaited provider shutdown and last native ID |

Inputs are JSON objects. `session_open` uses the common Due field names, but it
is a separate, deliberately smaller schema than a `due run --request` file.
There is no `--request` loader in this spike; pass arguments directly through MCP.
Unknown fields are rejected. `schema`, `native_args`, trace settings and profile
inheritance are not supported. `binary` selects a local provider executable; it
is mainly useful for fixtures and explicit local installs.

`effort` accepts `low`, `medium`, or `high`. Paths resolve from the server's
working directory. `timeout` accepts Due's duration syntax, such as `5m`, and
applies to each turn; there is no default turn deadline. Control operations and native graceful-shutdown budgets have separate
provider-specific deadlines. These are not an end-to-end shutdown bound: the
pinned Claude transport can stall on a blocked stdin write. `ephemeral` defaults
to false and conflicts with `resume`.

## Two turns

The examples below are `tools/call` arguments, not raw native provider messages.
Copy IDs from responses into subsequent calls.

```json
{"name":"session_open","arguments":{"provider":"claude","model":"sonnet","cwd":"/path/to/project","timeout":"5m"}}
```

Use `"provider":"codex"` and omit the Claude model to select Codex instead.
Open returns a Due handle in `session_id`; a native ID can be absent until the
first Claude turn. Start work:

```json
{"name":"turn_start","arguments":{"session_id":"DUE_HANDLE","prompt":"Review the parser. Do not change files."}}
{"name":"turn_read","arguments":{"session_id":"DUE_HANDLE","turn_id":"TURN_ID","after":0,"wait_ms":10000}}
```

Read again using the returned `cursor` as `after`. A read waits at most 10 seconds
and returns immediately for pending input or a terminal result. States are
`running`, `awaiting_input`, and `finished`. `finished` is not a success verdict:
inspect `result.status` and `result.error`. The final result uses Due's existing
result vocabulary, with its `run_id` set to the Due turn ID.

After reading the result, submit a second prompt to the same session:

```json
{"name":"turn_start","arguments":{"session_id":"DUE_HANDLE","prompt":"Which finding matters most?"}}
```

A second in-flight turn is rejected. `turn_read` retains only the latest turn's
result and event window. `session_read` retains a bounded item window spanning
turns. There is no automatic retry, queue or background job store.

## Pushed session items

The [small Python client](../examples/mcp-client/README.md) launches either provider,
runs two turns in one session and consumes the full item stream. It uses only
the Python standard library and includes provider-free tests.
The [JavaScript session comparison](../examples/session-harness/README.md) runs
the same correction workflow through CLI resume and this resident transport.

Clients that support Due's custom notification can opt in on open:

```json
{"name":"session_open","arguments":{"provider":"codex","cwd":"/path/to/project","stream":true}}
```

Register the notification handler before opening: startup items can arrive
before the `session_open` response. Notifications continue between tool calls
and do not require `turn_read`. A native tool item, for example:

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/due/session_item",
  "params": {
    "session_id": "DUE_HANDLE",
    "turn_id": "TURN_ID",
    "cursor": 18,
    "kind": "native",
    "data": {
      "provider": "codex",
      "method": "item/completed",
      "params": {
        "threadId": "NATIVE_THREAD_ID",
        "turnId": "NATIVE_TURN_ID",
        "item": {"id": "ITEM_ID", "type": "agentMessage", "text": "The parser rejects empty input."}
      }
    },
    "push_dropped": 0
  }
}
```

This is a Due-specific JSON-RPC notification over MCP stdio, not standard MCP
progress or logging. `stream` defaults to false. Generic MCP hosts may ignore
unknown notifications; use a client with a custom handler, or use
`session_read`. No initialization capability negotiation is implied by `stream`.

Each item has a session-wide increasing `cursor`. `turn_id` is a Due turn ID
only when attribution is known; startup, idle and unattributed native events
use null. The native payload keeps its own provider IDs and unknown fields.
Claude native records use `data: {"provider":"claude","event":{...}}`;
Codex records use `data: {"provider":"codex","method":"...","params":{...}}`.
The item kinds distinguish provenance:

| Kind | Contents |
| --- | --- |
| `native` | Complete native payload exposed by the wrapper, including message and tool items |
| `native_response` | Codex thread/start, thread/resume or turn/start response in `data.response`; structured RPC failures in `data.error` |
| `native_request` | Native requests rejected outside the pending-interaction flow |
| `native_summary` | Claude's projected initialization event or wrapper subscriber-loss marker |
| `native_snapshot` | Full Claude initialization recovered from the completed turn, delivered later |
| `turn_started` / `turn_finished` | Due-submitted prompt / normalized Due result |
| `interaction` / `interaction_resolved` | Pending request `{request_id,request}` / resolution `{request_id}` |
| `omitted` | Explicit record-size omission with original kind, byte count and reason |

The stream also includes Due session lifecycle and control records. This is
the complete payload available through the pinned wrappers, not a byte-for-byte
copy of their transports: Claude consumes control responses internally and
projects live initialization metadata. Its permission callback exposes the
request body rather than the whole control envelope. A delayed initialization
snapshot is available only when the wrapper retained it in a turn. Native resume
does not imply that Due can replay all earlier provider history.

Recover items after the last session cursor received, or read without push:

```json
{"name":"session_read","arguments":{"session_id":"DUE_HANDLE","after":18,"limit":64}}
```

The response contains `items`, `cursor` (the last returned position),
`latest_cursor`, `has_more`, `items_lost` and `dropped_items`. Advance `after` to
`cursor`; continue while `has_more` is true. `limit` defaults to 64 and accepts
1 through 256. This cursor is separate from `turn_read`'s per-turn cursor.
For example, after a second turn starts, `session_read` can still return the
first turn's terminal item while `turn_read` addresses only the second turn.

The journal holds at most 1024 records and 8 MiB. Individual records above
4 MiB become explicit `omitted` records. The independent bounded push queue
holds at most 256 items and 8 MiB and drops oldest items under backpressure; `push_dropped` is its cumulative
connection-wide drop count. A cursor gap or increased drop count tells a
streaming caller to use `session_read`; `items_lost` indicates that the journal
also lost requested payloads. This is an in-memory recovery window, not durable
storage. A blocked stdout write or final output drain has a five-second deadline;
expiration shuts down the connection and initiates provider cleanup. Read anything needed before `session_close` or process exit. Slow or
disconnected clients cannot rely on receiving every terminal notification;
`turn_read` still retains the current terminal result separately.

## Permissions and interruption

`turn_read.interactions` and pushed `interaction` items carry explicit native
requests with opaque IDs.
Requests are never automatically approved by Due. The caller must make the
decision within its own authorization. Already-allowed provider actions need
not produce requests.

A Claude denial:

```json
{"name":"interaction_reply","arguments":{"session_id":"DUE_HANDLE","turn_id":"TURN_ID","request_id":"REQUEST_ID","response":{"behavior":"deny","message":"This task is read-only."}}}
```

Claude also accepts `{"behavior":"allow"}` with optional `updated_input`.
Codex takes the native response object required by the request's method; command
approval commonly uses `{"decision":"decline"}` or `{"decision":"accept"}`.
Other native requests, including questions, have different response shapes.
The spike exposes their method and payload rather than translating all of them.
Unknown, resolved and completed-turn interaction IDs are rejected. Reply success
means delivery, not that the requested operation succeeded.

`turn_interrupt` requests a native interruption. Continue reading until the
turn settles before sending another prompt. An interruption does not roll back
changes. In particular, Codex turn interruption is not proof that a running
native command has stopped; graceful `session_close` shuts down App Server and
lets it settle commands. Steering is not exposed in this spike.

## Close, resume and limits

Save `native_session_id` from `turn_read` or `session_close` to resume later:

```json
{"name":"session_close","arguments":{"session_id":"DUE_HANDLE"}}
{"name":"session_open","arguments":{"provider":"claude","resume":"NATIVE_SESSION_ID"}}
```

Due handles and native IDs are different namespaces. Use the same provider and
available native session storage on resume. Ephemeral sessions cannot be
resumed. Settings are fixed for the open session; this is not a hot-reload API.

Closing MCP stdin, a transport failure, or SIGINT/SIGTERM/SIGHUP initiates
provider cleanup. No new sessions or turns are accepted after shutdown begins.
A deadline closes the native session; the failed Due handle must be closed
before opening another. Process exit and complete descendant termination are
subject to the underlying wrapper/platform contracts.
On platforms using the Tokio blocking-stdio fallback, including macOS, an
open stdin can keep the MCP process alive after signal-driven native cleanup.
Callers must close stdin to release that read. The RPC adapter uses dedicated
stdio bridges and tests this shutdown case explicitly.

The `turn_read` native event window holds at most 256 records and 1 MiB per turn. Use
`events_lost` and `dropped_events` to detect gaps; events are diagnostic, while
the retained terminal result and pending interactions are separate. Events can
include model text, tool inputs, paths and other sensitive native payloads.
This is intentionally different from Due's redacted optional run tracing.

The transport accepts input frames up to 1 MiB and at most 16 concurrent MCP
requests. Codex caps native output at 64 MiB per resident process and final
answer text at 4 MiB. The pinned Claude wrapper retains native turn events
internally without a configurable byte cap; the outward event window is not a
whole-process memory bound. Keep the spike to bounded tasks. Pending native
requests are capped at 32. No native wrapper revisions were changed.

Claude native cost is a session total, including spend restored on resume;
it is not automatically the cost of this turn. Codex cost remains absent.

## Validation

```sh
cargo test --locked --features mcp --test mcp
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo +1.90 check --locked --all-features
```

The stdio integration tests use a Bash fake provider with envelopes taken from
the pinned wrappers' fixtures. They cover both providers, multiple turns,
interrupt/reuse, permission replies, native errors, deadline settlement, EOF,
oversized input shutdown and early Codex completion. Push tests exercise live
delivery without polling, complete nested native payloads, explicit interactions,
default notification silence, terminal delivery after stdin EOF, reply ordering and
session recovery across turns with pagination.
Native tool-free checks
also verified two-turn memory and close/resume continuity on macOS with Claude
Code 2.1.316 and Codex CLI 0.165.0. Push-only callers additionally completed two
turns and recovered the session journal with both providers, without calling
`turn_read`. Native approval and tool-process termination behavior were not
exercised by those tool-free checks.
