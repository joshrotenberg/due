# Embedded clients on the RPC boundary

Decision: JavaScript is the supported harness language in Due 0.2.0. Use
[`due script`](harness.md) for product workflows. The report below records the
comparison that informed this choice; its test counts, versions and live evidence
are historical observations, not the current release validation summary. The
comparison runner now shares the production Rust/JavaScript implementation.

The current split is useful: Rust owns native sessions and the wire contract;
an optional language client gives scripts session operations and item callbacks.
A user can also implement the raw newline-delimited JSON-RPC protocol directly.
Embedding does not introduce a second provider API or require callers to use
Due's choice of scripting language.

The [comparison runner](../examples/rpc-languages/README.md) implements that
boundary with embedded Lua and QuickJS on the same Rust process/I/O bridge.
The comparison was isolated in a separate private crate before the language
decision. Production now includes JavaScript and RPC in normal builds, while
`due run` semantics, wrapper revisions and the separate MCP branch are preserved.
Inputs remain JSON.

## What was compared

Both clients implement request IDs and response routing in guest code, rather
than calling a complete Rust client through thin language bindings. Both support
resident multi-turn conversations, item callbacks, explicit permission replies,
deferred decisions, interruption and cleanup. The common Rust bridge captures
raw bytes before conversion, bounds frames and queues, handles signals and
settles the owned Due process.

The original synchronous slice uses callbacks and a cooperative pump. The
opt-in `--async` comparison now adds owned JavaScript Promises and Lua coroutines,
with the same Rust process supervisor beneath both. QuickJS is embedded
JavaScript, with no Node, npm or browser API;
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
| Async evaluation with `--async` | `async`/`await`, bounded microtask pump, tracked Promises and rejections | Managed coroutines suspend inside client methods; raw coroutine scheduling disabled |
| Independent verification | Shared Rust-owned command runner | Same command runner and limits |
| Synchronous compatibility | Promise use still rejected without `--async` | Callback yields still rejected without `--async` |

JavaScript remains the provisional lead for a JSON-facing client: callers can
work directly with the protocol's shapes, and the host needs less conversion
support. Lua remains viable and has a useful integer representation. The tests
show that most difficult behavior belongs to transport and session lifecycle,
not syntax. This was not a controlled comparison of agent authoring quality or
a runtime performance benchmark.

## Initial synchronous evidence from 2026-11-02

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

Validation of that initial synchronous slice passed 177 default Due tests, 225 RPC-enabled tests, all
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

## Async supervision evidence

Both embedded clients now perform the same bounded assignment through
`Due.run`: open one session from a JSON profile, send a turn, independently check
the changed artifact, and request a revision only when that check fails normally.
The [matching workflows](../examples/rpc-languages/README.md#async-supervision)
use caller-selected commands. A worker's claim that tests passed is not acceptance.

The deterministic peer keeps emitting items while a delayed permission command
or verifier runs. Shared scenarios cover continued delivery, interaction retirement,
turn completion during a callback, late permission acknowledgements, genuine and
stale reply errors, callback failure, caught cancellation, native failure during
verification, workflow expiry, unfinished turns and abandoned commands. Tests
inspect receipts, peer EOF and child liveness rather than only workflow exit codes.
They also send SIGTERM while a checker is running, retain a distinct callback
error after retirement, and run both checked-in verify/revise examples.
The actual Due server also passes both languages against both fake native adapters,
including profile loading, deferred denial, interruption and reuse. These async
checks make no model calls; the live conversations above tested the earlier
synchronous clients.

Final embedded validation passes 59 tests plus two explicit native integration
tests covering all eight sync/async language/provider combinations. Formatting,
all-target clippy with both engines and with each engine separately, the Rust 1.90
check and the release build pass. The unchanged default Due suite passes all
177 tests and its required pre-commit checks. These local checks ran on macOS.
The full RPC-enabled suite also passes all 251 tests and all-target clippy.

Rust owns command spawning, combined output limits, strict UTF-8 decoding,
deadlines and cancellation. Unix cleanup signals the owned process group before
reaping its leader and drains both output pipes. Its tests include a leader that
exits while descendants retain those pipes. macOS reports `EPERM` when signaling
a group containing only an unreaped exited leader; the implementation tolerates
that only after verifying the leader exited and no other group members remain.
Cleanup and receipt failures stay sticky even after a script consumes a result.

Permission work belongs to the supplied interaction signal. Retirement aborts
that signal and suppresses a late decision, but a distinct callback error still
fails the workflow. Already-sent replies retain their acknowledgement ownership.
Scope cancellation remains failure even when caught. On exit, managed callbacks,
commands and native-session cleanup must settle; an unfinished operation cannot
silently turn into success.

This comparison supports choosing JavaScript for caller-facing ergonomics: JSON
needs no table conventions and `async`/`await` makes suspension visible in the
workflow. The host implementation is not automatically simpler. QuickJS needs
bounded job pumping, rejection tracking and inspection of actual Promise state;
its resolve hook alone does not account for rejected Promises. Lua's restricted
managed coroutines make ownership easier to enumerate, at the cost of an explicit
JSON adapter and suspension hidden inside method calls. Both pass the same
supervision contract. Neither result measures agent authoring quality or speed.

## Selected harness

JavaScript is selected. [`due script`](harness.md) is the supported command, with
versioned JSON results, existing JSON profiles and embedded `supervise` guidance.
Normal builds include it and raw RPC; the delegate plugin routes callers to the
installed one-shot or supervision guidance. The shared private Rust harness
crate owns the processes and limits beneath both the product and comparison tests.

The feature freeze is lifted. Caller-facing assignment composition belongs in
JavaScript, while Rust retains provider lifecycle and raw RPC ownership. Use Due
on ordinary development projects and improve it from concrete failures and
friction. The former mandatory source-trial/reporting and A/B workflow is retired.

Other explicit limits remain: these clients fail on item gaps instead of
implementing cursor replay; interpreter deadlines are cooperative, including
Lua's non-preemptible library C calls; and the runner is for trusted scripts,
not an untrusted-code sandbox. Each scope owns one session; parallel workers
require separate scopes/processes, not a new server-side agent loop. Host verifier
commands are not constrained by native profile sandboxes. Unix descendants that
escape their owned process group are outside supervision; non-Unix group cleanup
is not implemented. The initial local async comparison evidence was macOS-only; current supported
builds require the repository's macOS/Linux CI checks. A public Rust RPC crate or
standalone client remains a packaging decision after the contract stabilizes.
