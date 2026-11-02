# Embedded clients on the RPC boundary

The current split is useful: Rust owns native sessions and the wire contract;
an optional language client gives scripts session operations and item callbacks.
A user can also implement the raw newline-delimited JSON-RPC protocol directly.
Embedding does not introduce a second provider API or require callers to use
Due's choice of scripting language.

The [comparison runner](../examples/rpc-languages/README.md) implements that
boundary with embedded Lua and QuickJS on the same Rust process/I/O bridge.
It is a separate private crate, with no changes to default Due dependencies,
`due run`, wrapper revisions or the separate MCP branch. Inputs remain JSON.

## What was compared

Both clients implement request IDs and response routing in guest code, rather
than calling a complete Rust client through thin language bindings. Both support
resident multi-turn conversations, item callbacks, explicit permission replies,
deferred decisions, interruption and cleanup. The common Rust bridge captures
raw bytes before conversion, bounds frames and queues, handles signals and
settles the owned Due process.

This first slice uses synchronous callbacks and a cooperative pump. It deliberately
does not establish which language is better for concurrent asynchronous workflow
composition. QuickJS is embedded JavaScript, with no Node, npm or browser API;
Lua has selected standard libraries and the explicit bridge/JSON helpers.
See the [QuickJS binding documentation](https://docs.rs/rquickjs/0.14.0/rquickjs/)
and [Lua language model](https://www.lua.org/manual/5.4/manual.html#2.1).

| Concern | JavaScript | Lua |
| --- | --- | --- |
| Incoming JSON shapes | Native arrays, objects and null | Host supplies array identity and `json.null` |
| Outbound values | Checked serializer rejects unsafe/non-JSON values | Rust conversion checks table shape, keys, cycles and numeric values |
| Integer precision | `9007199254740993` rounds in the decoded view | Signed 64-bit integer remains exact |
| Universally lossless numbers | No | No; unsigned values above signed range can become floats |
| Raw evidence | Identical Rust byte capture | Identical Rust byte capture |
| Routing, permissions, lifecycle | Implemented in client | Implemented in client |
| Async evaluation in this slice | Promise use rejected | Callback yields rejected; detached coroutines not supervised |

JavaScript remains the provisional lead for a JSON-facing client: callers can
work directly with the protocol's shapes, and the host needs less conversion
support. Lua remains viable and has a useful integer representation. The tests
show that most difficult behavior belongs to transport and session lifecycle,
not syntax. This was not a controlled comparison of agent authoring quality or
a runtime performance benchmark.

## Evidence from 2026-11-02

The shared deterministic peer exercises early startup items, terminal items before
start replies, reversed response ordering, ordinary RPC errors, failed model
turns, denial, deferred permissions, interrupt/reuse, stream loss, malformed JSON
and UTF-8, callback errors, sticky failures, deadlines and signals. Both languages
must preserve the original raw integers, nulls and empty arrays/objects. Tests
also verify actual child settlement, not just a returned client error.

Two bounded live conversations ran through the embedded runner on macOS arm64,
using the final candidate binaries after the interruption repair (`6f14ab1`):

| Embedded client | Provider | Turns | Session items | Native items | Result |
| --- | --- | --- | --- | --- | --- |
| QuickJS | Claude, requested `sonnet` / low effort | 2 | 40 | 30 | Marker recalled |
| Lua | Codex, requested `gpt-6-astra` / low effort | 2 | 81 | 72 | Marker recalled |

Both had continuous cursors, zero push drops, a final `session_closed` item,
exit code 0 and no forced kill or transport error. These prove basic live
multi-turn operation; provider timing and item counts are not language benchmarks.
Permission and failure cases are covered by deterministic peers, not those two
simple conversations. Private raw evidence, receipts and SHA-256 manifests are
retained under `/private/tmp/due-rpc-languages-20261102/live` in the `*-final`
directories. Earlier smoke runs against `cf5cb86` also passed.

The final validation passed 177 default Due tests, 225 RPC-enabled tests, all
104 existing Node client tests, and 23 embedded-runner tests. The explicit
native-adapter integration test also passed all four language/provider
combinations against the actual Due server with fake native binaries. Formatting,
clippy (default/RPC and both interpreter feature selections), Rust 1.90 checks,
and release builds passed.

Separate release builds with the same Rust 1.99.1 toolchain on macOS arm64 were
2,306,816 bytes for Lua and 2,972,400 bytes for QuickJS. These are standalone
runner binary sizes with default release settings, not Due binary sizes,
resident memory measurements or runtime speed comparisons.

The implementation itself was also exercised through existing resident RPC
sessions: Claude implemented/reviewed the JS client; Codex implemented/reviewed
the Lua client and the startup-interrupt repair. Parent review and shared tests
checked their changes independently. These coding sessions used the existing
Node caller, not the new embedded clients. Two Claude turns and five Codex turns
kept one native session per provider, delivered 6,319 and 2,547 items respectively,
and closed without cursor gaps, push loss or omitted items. Claude required 21
explicit permission decisions; scratch-file paths were kept within its disposable
checkout. No blanket permission change was made.

## Findings that changed the implementation

- Lua initially copied every incoming item by re-encoding JSON. That applied
  outbound numeric restrictions to received native data. It now copies only
  callback-sensitive lifecycle state structurally and passes native payloads
  through without an encoding round trip.
- A callback must not mutate the client's private turn result or permission
  identity. Terminal state and interaction retirement belong before callback
  delivery, while the original lifecycle item must still reach the observer.
- QuickJS timeout values need explicit null conversion; Rust `Option::None`
  otherwise becomes JavaScript undefined. Runtime job inspection must also
  respect the binding's context lock. The adapter tests cover both paths.
- A discarded rejected Promise can leave no pending job. The synchronous
  experiment detects Promise creation rather than accidentally reporting
  success for abandoned asynchronous work.
- Actual Due integration exposed a Codex interruption race before its native
  turn ID arrived. Retaining the request requires an absolute deadline,
  ownership by the exact Due turn and an acknowledgement wait outside the
  manager lock. A later turn must never inherit a stale stop request. A second
  regression caught turn completion preceding the interrupt acknowledgement;
  the control now has bounded owner-level lifetime independent of the active
  turn, with late replies attributed to its original immutable IDs.

## What remains before choosing a harness

The next discriminating comparison is the same asynchronous workflow in both
languages: keep streaming while a permission callback awaits another operation,
cancel that operation when its turn ends, and join all spawned work before
returning. That means testing Lua coroutine composition against JavaScript
Promises on the same asynchronous Rust bridge, with identical ownership and
cleanup rules. The earlier workflow spikes contain useful patterns, but this
experiment does not import their scheduler.

Other explicit limits remain: these clients fail on item gaps instead of
implementing cursor replay; interpreter deadlines are cooperative, including
Lua's non-preemptible library C calls; and the runner is for trusted scripts,
not an untrusted-code sandbox. A public Rust RPC crate or shipped standalone
client can be decided after the client contract stabilizes. Neither requires
choosing the embedding language first.
