# Plain JSON-RPC stdio spike

Build only the RPC adapter, then launch it as a child process:

```sh
cargo build --locked --release --features rpc
./target/release/due rpc
```

This opt-in interface hosts a resident Claude `DuplexSession` or Codex
`AppServer`, with one session per Due process and one active turn at a time.
The separate [MCP experiment](https://github.com/joshrotenberg/due/tree/codex/due-mcp-spike)
lives on `codex/due-mcp-spike`; this branch contains only the RPC adapter.
Default builds and `due run` behavior remain unchanged; wrapper revisions stay
pinned. There is no model loop, scheduler, config loader or new configuration
format. Native authentication, settings and permissions remain inherited.

The optional [embedded-client comparison](../examples/rpc-languages/README.md)
runs Lua and QuickJS over one Rust raw-frame bridge. Each language implements
its own client above this wire contract; external callers can implement the
same operations without embedding either runtime.

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
| `session.close` | `session_id` | Awaited native shutdown, last native ID and first session failure, if any |

Parameters are JSON objects, with a deliberately smaller schema than a
`due run --request` file. Pass them directly through RPC; there is no request-file
loader. Unknown fields are rejected. `schema`, `native_args`, trace settings and
Due request-file inheritance are not exposed. `effort` is low/medium/high;
`timeout` is a per-turn duration such as `2m`, with no default deadline.
`ephemeral` defaults to false and conflicts with `resume`. Paths resolve from
the server's working directory; `binary` selects a local provider executable.
Settings remain fixed for the open session.

Due session handles and native provider IDs are different namespaces. Use the
returned `session_id` for RPC calls. Save `native_session_id` from `turn.read`
or `session.close` for a later `session.open` with `resume`. Native IDs can be
absent until Claude's first turn. Resume requires the same provider and its
persisted session storage; ephemeral sessions cannot be resumed.

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
handler before open: startup items can precede its response. If opening fails,
those items (including `session_failed`) carry a provisional session ID for
diagnostics, not a usable handle. Only a successful open publishes a handle;
another open may be attempted after the failed request settles. Use the returned
Due handle in subsequent requests:

```json
{"jsonrpc":"2.0","id":2,"method":"turn.start","params":{"session_id":"SESSION","prompt":"Review the parser. Do not change files."}}
```

This returns `result.turn_id`. A second in-flight turn is rejected. Native
items arrive independently, retaining
their provider IDs, nested payloads and unknown fields:

```json
{"jsonrpc":"2.0","method":"session.item","params":{"session_id":"SESSION","turn_id":"TURN","cursor":8,"kind":"native","data":{"provider":"codex","method":"item/completed","params":{"threadId":"NATIVE_THREAD","turnId":"NATIVE_TURN","item":{"id":"ITEM","type":"agentMessage","text":"The parser rejects empty input."}}},"push_dropped":0}}
```

Wait for `kind: "turn_finished"` for that turn; its `data` is the normalized Due
result. Alternatively, call `turn.read` with `session_id`, `turn_id`, `after: 0`
and `wait_ms: 10000`, then advance `after` to its returned `cursor`. Each read
waits at most 10 seconds and returns immediately for pending input or a terminal
result. States are `running`, `awaiting_input` and `finished`; inspect the final
`result.status` and `result.error` for success or failure. The normalized result
uses Due's existing vocabulary, with `run_id` set to the Due turn ID.

After the turn settles, send a follow-up through the same handle:

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

Close reports provider cleanup separately from an earlier session failure. When
the backend's close operation succeeds, its result contains `closed: true`,
`native_session_id` and `session_error` (the first recorded failure, or null).
The `session_closed` item carries the same ID and failure, plus `error` for the
cleanup outcome. A cleanup error still returns RPC error `-32000`; its lifecycle
item preserves those identifiers. Native wrapper shutdown errors are preserved,
including a nonzero child exit reported by Codex. Successful cleanup does not
make a failed turn successful. Successful cleanup cancels any unfinished turn.

Reads remain available while provider controls or close are awaiting native
completion, subject to the connection's request/output limits. After close
settles, the handle is removed even if cleanup failed; new reads reject it.
Reads already admitted may finish using their retained view. Save required
items and identifiers before closing.

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

`session.item` carries the complete payload available through the pinned
wrappers. Each item has a session-wide increasing `cursor`. `turn_id` is a Due
turn ID only when attribution is known; startup, idle and unattributed native
events use null. Native payloads retain their own provider IDs and unknown
fields. Claude records use `data: {"provider":"claude","event":{...}}`; Codex
records use `data: {"provider":"codex","method":"...","params":{...}}`.

| Kind | Contents |
| --- | --- |
| `native` | Complete exposed native payload, including message and tool items |
| `native_response` | Codex thread/start, thread/resume or turn/start response in `data.response`; structured RPC failures in `data.error` |
| `native_request` | All Codex native requests, including those later exposed as `interaction`; Claude permission requests denied outside the pending-interaction flow |
| `native_summary` | Claude's projected initialization event or wrapper subscriber-loss marker |
| `native_snapshot` | Full Claude initialization recovered from the completed turn, delivered later |
| `turn_started` / `turn_finished` | Due-submitted prompt / normalized Due result |
| `interaction` / `interaction_resolved` | Pending request `{request_id,request}` / resolution `{request_id}` |
| `omitted` | Explicit record-size omission with original kind, byte count and reason |

The stream also includes Due session lifecycle and control records. It is not a
byte-for-byte copy of the native protocol: Claude consumes control responses
internally and projects live initialization metadata. Its permission callback
exposes the request body rather than the whole control envelope. A delayed
initialization snapshot is available only when retained in a turn. Native
resume does not replay all earlier provider history through Due.

Streaming is opt-in: `stream` controls pushed notifications, not native event
collection. Claude partial-message events are requested and retained for polling
even with `stream: false`. `session.read` works without pushes and recovers gaps
using the last session-wide `cursor`. Keep that cursor separate from
`turn.read`'s cursor.
For example:

```json
{"jsonrpc":"2.0","id":5,"method":"session.read","params":{"session_id":"SESSION","after":18,"limit":64}}
```

The result contains `items`, `cursor` (the last returned position),
`latest_cursor`, `has_more`, `items_lost` and `dropped_items`. Advance `after` to
`cursor` and continue while `has_more` is true. `limit` defaults to 64 and accepts
1 through 256. The journal spans turns and retains 1024 items / 8 MiB; items
above 4 MiB become explicit `omitted` records. The independent push queue retains
256 items / 8 MiB and drops oldest items under backpressure. A cursor gap or
increased `push_dropped` count signals that the caller should recover through
`session.read`; `items_lost` indicates the journal also lost requested payloads.
This is bounded in-memory recovery, not durable delivery. Read anything needed
before closing the session or process.

`turn.read` retains only the latest turn's result and event window. Its native
event window holds 256 records / 1 MiB; `events_lost` and `dropped_events` report
gaps. The retained terminal result and pending interactions are separate, so a
caller can still read the current result after missing a pushed terminal item.
Native payloads can contain model text, tool inputs, paths and other sensitive
data, unlike Due's redacted optional run tracing.

An `interaction` item contains a native `request_id` and request body. Reply
using `interaction.reply` with that ID, the Due session and turn IDs, and a
provider-native response. This is a normal caller-initiated RPC request with
its own separate transport ID. Due never automatically approves requests.
Already-allowed native actions need not ask. Claude accepts
`{"behavior":"deny","message":"This task is read-only."}` or `{"behavior":"allow"}`
with optional `updated_input`. Codex uses the native response shape for the
request method; command approvals commonly accept `{"decision":"decline"}` or
`{"decision":"accept"}`, while questions have different shapes. Unknown,
resolved and completed-turn interaction IDs are rejected. Reply success means
delivery, not that the native operation succeeded.

`turn.interrupt` requests a stop; await its response and continue reading until
the turn settles before sending the next prompt. Codex retains an interruption
requested before its native turn ID arrives and emits `interrupt_pending`.
That item records intent, not native acceptance. The RPC response waits for the
native acknowledgement, even if the turn finishes first; late acknowledgements
remain attributed to the original Due turn. Only one Codex interruption may be
pending per session. Reads, permission replies and close remain available while
it waits, and another turn can start after the previous turn settles.

The Codex control deadline is 30 seconds from admission. Unsent intent is removed
on expiry or when its turn ends. A timeout after dispatch means acknowledgement
is unknown; the stop may already have been delivered. Close rejects the pending
response before native shutdown. Interruption does not roll back effects or
prove every native command stopped.

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
The concurrency error `-32001` is a recoverable admission rejection only while
output capacity remains available. Parse/admission error responses also consume
that capacity, and an in-flight response retains its byte budget until flushed.
The request limit does not reserve room for an unlimited stream of errors: a
caller that stops reading can hit the hard output limit and lose the connection.
A burst of immediate errors can also fill the queue before the writer runs,
even when the caller is reading.

Malformed JSON within a valid UTF-8 line receives `-32700` and leaves the
connection usable. Invalid UTF-8 or an oversized input line is a framing error
and ends the connection. There is no attempt to resynchronize after invalid
byte framing.

EOF, SIGINT/SIGTERM/SIGHUP, invalid framing or failed output starts native
cleanup and rejects new sessions/turns. Queued terminal items are drained when
the pipe remains usable. Shutdown awaits provider-owning operations; it is not
a five-second end-to-end guarantee. An already-running native open can consume
its provider control deadlines before cleanup, and the pinned Claude transport
can stall on a blocked native stdin write. Underlying wrapper/platform limits
on descendant settlement remain unchanged. Native turn event retention is also
not fully bounded by Due's outward journal limits. A native deadline closes the
session; close its failed Due handle before opening another. Codex caps native
output at 64 MiB per resident process and final answer text at 4 MiB. The pinned
Claude wrapper retains native turn events without a configurable byte cap.
Pending native requests are capped at 32. Claude denies excess permission
requests and keeps the session running. Codex rejects an excess native request
and fails the session, both before and after its native turn ID arrives. These
policies differ because Claude exposes typed permission decisions while Codex
exposes a broader native request stream. Claude's native cost is a session total,
including spend restored on resume; Codex cost remains absent.

## Rust host and language client

The current split has two responsibilities:

- The Rust session host owns native provider processes, turn admission,
  permission-request correlation, normalized results, bounded journals and
  provider cleanup. The RPC adapter adds framing, request dispatch and pushed
  session items. Codex notification turn IDs must agree before a payload can
  update a turn; rejected payloads remain observable as unattributed native
  evidence.
- The language client owns the Due child, request IDs, stream consumption,
  recovery, caller callbacks, caller wait policy and saved receipts. It must
  propagate session failures even between turns, stop starting permission
  callbacks after failure, and preserve evidence through cleanup. Parsed values
  also inherit that language's representation limits.

The Node example is a small standalone client of this contract. An eventual
embedded JavaScript or Lua client could use the same session operations through
stdio or an in-process binding to the Rust host. The session host should remain
the authority for provider lifecycle and turn state in either arrangement.
Embedding, a public Rust library and a packaged client are future choices; this
spike does not implement them or choose a language engine. Workflow composition
can remain ordinary caller code over the session API.

## Checking the spike

```sh
cargo test --locked --features rpc --test rpc
cargo clippy --locked --all-targets --features rpc -- -D warnings
cargo test --locked --all-features
cargo +1.90 check --locked --features rpc
node --test examples/session-harness/*.test.mjs
```

The offline tests use fake native peers for both providers. They cover session
continuity, full live items, cursor replay, permission overflow, startup and late
interrupt acknowledgements, reads during controls, failed-session cleanup,
deadlines, early results, framing and output saturation. No model credentials
are used by those tests. CI enables RPC on Linux and macOS. Process tests use
both pipes and Unix sockets, exercising Linux's pipe path and the fallback used
by Node-style socket-backed stdio.

To compare CLI resume with RPC using a real provider and a disposable workspace:

```sh
cargo build --locked --release --features rpc
node examples/session-harness/compare.mjs --due ./target/release/due \
  --provider codex --model gpt-6-astra --cwd /path/to/disposable-workspace \
  --artifacts /path/to/fresh-comparison --backend all
```

RPC keeps the external wire exchange small; callers still handle lifecycle,
permissions and recovery. Embedded Lua/JavaScript bindings could call the
session host directly without a wire protocol. This spike does not implement
those bindings.

The observations below were recorded before the transport branches split.
Their commit IDs, binaries and test counts refer to those historical snapshots.

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

## Committed-client acceptance (2026-11-02)

The client at `b146fd6` completed two real Claude coding turns using the
repository's unmodified `withSession`, `turnWaitMs: null`, and `onInteraction`.
Claude Code 2.1.316 (`sonnet`, medium effort) reviewed the wait change, ran the
two delayed-turn acceptance tests, and added a monitored-caller documentation
example in a disposable checkout. One Due process and one native Claude session
ID covered both turns. Both succeeded; close exited 0 with no forced kill,
restart, resume, body error or cleanup error.

The caller retained 663 items with continuous cursors, zero reported push drops,
no omitted items or native loss markers, and the final session-closed item.
It sent one `session.open`, two `turn.start`, five `interaction.reply` requests
and one `session.close`, with no polling. Permissions used the committed
callback API; no private client or transport patch was loaded.

The receipt recorded `turn_wait_ms: null`. The turns took about 203 and 196
seconds, respectively, so neither exceeded the previous five-minute default.
Offline checks separately exercised short finite waits, delayed turns with
waiting disabled, and cancellation. All 146 client tests passed, and both
README JavaScript examples passed syntax checks. These observations establish
the public caller workflow on local macOS; they are not a timing benchmark.

## Backlog and edge-case work (2026-11-02)

The RPC-only snapshot `f08d093` ran two resident coding conversations on macOS:
Claude Code 2.1.316 (`sonnet`) and Codex CLI 0.165.0 (`gpt-6-astra`), both with
requested medium effort. Each completed four turns through one Due process and
one native session, using the committed Node client with `turnWaitMs: null`.
Three Claude turns exceeded five minutes while the caller actively monitored
work and permission requests. These were different assignments, not a provider
performance comparison.

Backlog triage confirmed that the reported fixes for [#29](https://github.com/joshrotenberg/due/issues/29),
[#30](https://github.com/joshrotenberg/due/issues/30) and
[#31](https://github.com/joshrotenberg/due/issues/31) were already in the checkout.
The workers used those failure themes to investigate the resident host and client:

- Codex reproduced contradictory native turn IDs producing a successful result.
  The host now rejects conflicting or malformed supplied IDs before they update
  accepted answer/usage state. Raw rejected evidence remains available without
  being attributed to the active turn. Missing and optional null identities,
  normal early delivery, and unrelated threads retain their intended behavior.
- Claude reproduced invalid UTF-8 being silently replaced and permission
  callbacks starting after a fatal client error. Strict decoding now rejects
  corrupted frames; failed/closing sessions do not start new decisions, while
  item draining preserves cleanup evidence.
- An independent actual-Due/fake-native probe showed that a `session_failed`
  between turns could be ignored by the client. It now fails the scope and keeps
  the failure receipt without rewriting an earlier successful turn.
- Large JSON integers exposed a language representation limit: parsed JavaScript
  values can round native numbers. The client now retains bounded original
  `stdout.bin` bytes before decoding, alongside its parsed convenience views.
  A real 64 MiB capture-overflow regression verifies failure and cleanup.
- Codex's client review and an independent caller probe found descriptor leaks
  when an artifact already existed. Prelaunch resource acquisition now closes
  descriptors opened by that attempt if a later open or synchronous spawn fails.
  Existing evidence remains unchanged.

The caller captured 3276 Claude items and 1907 Codex items, with continuous
cursors, zero reported push drops, no omitted items and both final
`session_closed` records. Claude received 15 explicit permission replies:
14 allowed and one denied; after the denial it revised the proposed edit in the
same turn. Codex inherited native settings and asked for no permission replies;
its thread response reported `approvalPolicy: never` and `dangerFullAccess`.
This run does not establish equivalence between provider permission policies.
Both Due processes exited 0 without forced kills, body errors or cleanup errors.
Neither client polled `turn.read` or `session.read`.

Live coding used the original `f08d093` transport/client; the discovered defects
and final repairs were exercised with deterministic fake peers. Final local
checks passed 177 default Rust tests, 220 RPC-enabled Rust tests and 104 Node
client tests, plus formatting, clippy, Rust 1.90 checks and release builds.
Independent review corrected malformed-ID handling and strengthened request-log
and split-Unicode assertions. These observations cover local macOS behavior;
no hosted or other-platform acceptance is implied.
