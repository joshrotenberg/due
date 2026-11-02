# Plain JSON-RPC stdio spike

Build only the RPC adapter, then launch it as a child process:

```sh
cargo build --locked --release --features rpc
./target/release/due rpc
```

This is an opt-in alternative to `due mcp`. It uses the same resident Claude
`DuplexSession` and Codex `AppServer` host, with one session per Due process and
one active turn at a time. The `rpc` feature does not depend on `tower-mcp`.
Default builds and `due run` behavior remain unchanged; wrapper revisions stay
pinned. There is no model loop, scheduler, config loader or new configuration
format. Native authentication, settings and permissions remain inherited.

## Wire contract

Send compact UTF-8 JSON objects, one per newline, through stdin. Read responses
and `session.item` notifications continuously from stdout; diagnostics use
stderr. There is no initialization handshake, tool discovery or `tools/call`
envelope. `session.open` reports `protocol_version: 1`. Check it before starting
work; close the connection if unsupported.

This small JSON-RPC 2.0 profile supports named parameters, string/integer/null
request IDs and notifications (an absent `id`). Batches and positional
parameters are intentionally unsupported. Use unique IDs for concurrent calls.
Replies may arrive out of order. A notification receives no response, including
on failure; use requests for session mutations whose outcome matters.

| Method | Parameters | Result |
| --- | --- | --- |
| `session.open` | `provider`; optional `model`, `effort`, `cwd`, `binary`, `resume`, `ephemeral`, `timeout`, `stream` | Protocol version, Due handle, available native ID, capabilities |
| `turn.start` | `session_id`, `prompt` | Due turn ID immediately |
| `turn.read` | `session_id`, `turn_id`; optional `after`, `wait_ms` | Turn state, native event window, pending interactions, final result |
| `session.read` | `session_id`; optional `after`, `limit` | Session items across turns, cursor and loss indicators |
| `turn.interrupt` | `session_id`, `turn_id` | Interruption requested; wait for the terminal result |
| `interaction.reply` | `session_id`, `turn_id`, `request_id`, `response` | Native reply accepted for delivery |
| `session.close` | `session_id` | Awaited native shutdown and last native ID |

Parameters and results otherwise match the [shared session contract](mcp.md).
Unknown parameter fields are rejected. `schema`, `native_args`, trace settings
and Due request-file inheritance are not exposed. `effort` is low/medium/high;
`timeout` is a per-turn duration such as `2m`, with no default deadline.
`ephemeral` defaults to false and conflicts with `resume`.

RPC failures use an `error` object with a code and message: parse error `-32700`,
invalid request or unsupported batch `-32600`, unknown method `-32601`, invalid
parameters `-32602`, session operation failure `-32000`, concurrency limit
`-32001`. An ordinary failed model turn is a successful RPC operation containing
a Due result with `status: "failed"`; inspect the result status, not merely the
absence of an RPC error. There are no automatic retries.

## A conversation

Open a session with streaming enabled:

```json
{"jsonrpc":"2.0","id":1,"method":"session.open","params":{"provider":"codex","cwd":"/path/to/project","timeout":"2m","stream":true}}
```

The response's `result` contains `session_id`, `protocol_version: 1` and
`stream: {"enabled":true,"method":"session.item"}`. Register the notification
handler before open: startup items can precede its response. Use the returned
Due handle in subsequent requests:

```json
{"jsonrpc":"2.0","id":2,"method":"turn.start","params":{"session_id":"SESSION","prompt":"Review the parser. Do not change files."}}
```

This returns `result.turn_id`. Native items arrive independently, retaining
their provider IDs, nested payloads and unknown fields:

```json
{"jsonrpc":"2.0","method":"session.item","params":{"session_id":"SESSION","turn_id":"TURN","cursor":8,"kind":"native","data":{"provider":"codex","method":"item/completed","params":{"threadId":"NATIVE_THREAD","turnId":"NATIVE_TURN","item":{"id":"ITEM","type":"agentMessage","text":"The parser rejects empty input."}}},"push_dropped":0}}
```

Wait for `kind: "turn_finished"` for that turn; its `data` is the normalized Due
result. Then send a follow-up through the same handle:

```json
{"jsonrpc":"2.0","id":3,"method":"turn.start","params":{"session_id":"SESSION","prompt":"Which finding matters most?"}}
```

After its terminal result, read any needed journal items and close:

```json
{"jsonrpc":"2.0","id":4,"method":"session.close","params":{"session_id":"SESSION"}}
```

Continue reading stdout during close and through EOF, then await the Due child
exit. A terminal item may precede the `turn.start` response; clients need to
correlate by IDs rather than assuming response-first ordering.

The runnable [JavaScript example](../examples/session-harness/rpc-example.mjs)
does two turns, independently checks both answers and writes all observed
session items to stdout:

```sh
node examples/session-harness/rpc-example.mjs \
  --due ./target/release/due --provider codex --model gpt-6-astra \
  --cwd /path/to/disposable-workspace --artifacts /path/to/fresh-artifacts \
  > /path/to/session-items.jsonl
```

Use `--provider claude --model sonnet` for Claude. The shared
[harness](../examples/session-harness/README.md) selects this transport with
`withSession({backend: 'rpc', ...}, callback)`. Its compact example rejects
unexpected permission requests and cleans up; it does not silently approve them.
The harness accepts top-level `turnWaitMs` for its terminal-result wait: omit it
for five minutes, choose a positive integer number of milliseconds up to
2147483647, or use `null` for actively monitored work without a client turn
deadline. Native `profile.timeout`, control-request waits and shutdown bounds
remain separate. See the [caller wait options](../examples/session-harness/README.md#caller-turn-waits).

## Streaming, permissions and cleanup

`session.item` carries the same complete available wrapper payloads as the MCP
adapter. It is not a byte-for-byte copy of the native protocol: Claude exposes
projected initialization metadata live and may provide the complete snapshot
later, and its internal control responses are consumed by the wrapper. See the
[item provenance and limits](mcp.md#pushed-session-items). Codex records every native
request as `native_request` before correlation, including requests later exposed
as `interaction`. Claude uses `native_request` for permission requests denied
outside its pending-interaction flow.

Streaming is opt-in. `session.read` works without it and recovers gaps using the
last session-wide `cursor`. Keep that cursor separate from `turn.read`'s cursor.
The journal retains 1024 items / 8 MiB; items above 4 MiB become explicit
`omitted` records. The independent push queue retains 256 items / 8 MiB and
drops oldest items under backpressure. Inspect `push_dropped`, `items_lost` and
`dropped_items`; this is bounded in-memory recovery, not durable delivery.

An `interaction` item contains a native `request_id` and request body. Reply
using `interaction.reply` with that ID, the Due session and turn IDs, and a
provider-native response. This is a normal caller-initiated RPC request with
its own separate transport ID. Due never automatically approves requests.
Already-allowed native actions need not ask. `turn.interrupt` requests a stop;
continue reading until the turn settles before sending the next prompt.
Interruption does not roll back effects or prove every native command stopped.

The optional Node harness (`examples/session-harness`) rejects interactions by
default. Its caller-supplied `onInteraction(record, signal)` may be async and
returns the native response; the harness sends it with the IDs from the item,
discards any result produced after the interaction resolved, the turn finished,
the session failed or the scope closed, and treats a stale-reply error as a race
only after observing that exact interaction's resolution or its turn's terminal
item. Other reply failures remain session failures. A callback that settles
after its scope has closed cannot change the saved receipt. See its README.

Input frames are bounded to 1 MiB and concurrent operations to 16. The sole
stdout writer interleaves responses and items without interleaving their bytes.
Responses have an 8 MiB individual/aggregate queued-byte limit and a queue of 16;
overflow ends the connection and starts cleanup rather than silently losing a
response. A blocked write or final drain has a five-second deadline.

EOF, SIGINT/SIGTERM/SIGHUP, invalid framing or failed output starts native
cleanup and rejects new sessions/turns. Queued terminal items are drained when
the pipe remains usable. Shutdown awaits provider-owning operations; it is not
a five-second end-to-end guarantee. An already-running native open can consume
its provider control deadlines before cleanup, and the pinned Claude transport
can stall on a blocked native stdin write. Underlying wrapper/platform limits
on descendant settlement remain unchanged. Native turn event retention is also
not fully bounded by Due's outward journal limits.

## Checking the spike

```sh
cargo test --locked --features rpc --test rpc
cargo clippy --locked --all-targets --features rpc -- -D warnings
cargo test --locked --all-features
cargo +1.90 check --locked --features rpc
node --test examples/session-harness/*.test.mjs
```

The offline tests use fake native peers for both providers. They cover session
continuity, full live items, cursor replay, permissions, interrupt during a
concurrent long poll, native failures, deadlines, early results, framing and
cleanup. No model credentials are used by those tests.

To compare all three clients with a real provider and a disposable workspace:

```sh
cargo build --locked --release --features mcp,rpc
node examples/session-harness/compare.mjs --due ./target/release/due \
  --provider codex --model gpt-6-astra --cwd /path/to/disposable-workspace \
  --artifacts /path/to/fresh-comparison --backend all
```

Both persistent adapters call the same session host. RPC simplifies the external
wire exchange; it does not remove lifecycle, permission or recovery handling.
MCP retains standard tool discovery and host integration. Embedded Lua/JavaScript
bindings can call the shared host directly without routing through either wire
protocol. This spike does not implement those bindings.

## Live observation (2026-11-01)

The RPC-only release binary completed the two-turn correction with Claude Code
2.1.316 (`sonnet`) and Codex CLI 0.165.0 (`gpt-6-astra`) on macOS arm64 / Node
26.12.1. Both used low effort and a two-minute native turn deadline. Each
preserved one native session ID, remembered the original marker/order details,
and produced independently checked totals 19 then 17.

Claude delivered 43 session items; Codex delivered 99. Both captures had
consecutive cursors, zero push drops, two terminal results and the final
session-closed item. Both Due processes exited 0, and the disposable workspace
remained unchanged. Each client sent exactly `session.open`, two `turn.start`
requests and `session.close`; neither needed polling or an initialization call.

These were tool-free checks with inherited native permissions, not live
permission or native tool-process termination tests. No performance comparison
is claimed. The binary was built from base `a303cc89` plus the uncommitted
session/MCP/RPC spike. Its SHA-256 was
`e1189b454f3a4919cd71073134674697932f09323eae79fc8b685236555a78d6`.

Offline checks passed the default Due suite (177 tests), the all-features suite
(228 tests before the final stalled-output regression), all 14 RPC integration
tests, and 59 JavaScript tests. The added shutdown regressions hold stdin open
while testing SIGTERM, broken stdout and unread stdout. Format, clippy and Rust
1.90 checks passed; Linux/Windows native process behavior was not exercised.

## Real-work observation (2026-11-02 UTC)

One Codex RPC session (`gpt-6-astra`, medium effort) worked across three
turns in a disposable Due snapshot: preserving process receipts after client
failure, correcting early native-request correlation, and addressing review
feedback while retaining the 32-request bound. All three turns preserved the
same native session ID. The caller retained 1438 session items with consecutive
cursors and zero reported push drops, received the session-closed item, and
observed Due exit 0 without a forced kill. Independent review and subsequent
checks passed 233 Rust tests and 65 JavaScript tests. This was one local macOS
run using the pre-fix RPC binary; the fixes were checked through offline
regressions, not live native approval tests. Native permissions were inherited
(`approval: never`, `sandbox: dangerFullAccess`), so this was not a
sandbox-boundary test.

## Claude coding trial (2026-11-02 UTC)

Claude Code 2.1.316 (`sonnet`, medium effort) worked on the Node session client
in a disposable Due checkout based on `3674146`. A caller-owned permission
bridge reviewed concrete native requests and sent 26 `interaction.reply`
requests; native settings were inherited, with no bypass mode added.

The first caller hit its 15-minute wait limit before the task finished. Due
cancelled the turn and exited 0 without a forced kill. After inspecting partial
edits, the caller explicitly resumed the same native Claude conversation in a
second Due process, with an actively monitored caller and no fixed turn wait.
That process completed three coding turns and closed cleanly. This used one
native conversation across two RPC processes.

Across both captures, the caller retained 5295 session items with consecutive
cursors within each process, zero reported push drops, no omitted items, and
both final session-closed items. No journal polling was needed. The initial
failed receipt lost the native ID even though initialization and the cancelled
terminal item retained it; the stream supplied the ID used for explicit resume.

The work added explicit asynchronous permission decisions to the Node client,
corrected permission resolution and failure receipts, and retained fresh Claude
initialization IDs before observer callbacks. Independent reproductions and
132 offline Node tests passed. The live run used a caller-only permission bridge
and the pre-fix client; the final example changes were verified offline. These
are local macOS observations, not a performance comparison or evidence of other
platforms' native process behavior. At that point the checked-in example still
used a fixed terminal wait; that trial did not change it.
