# A session API over CLI resume and resident execution

This optional Node.js 22+ experiment runs the same JavaScript correction workflow
through CLI resume or resident plain JSON-RPC. The JavaScript uses
Node built-ins only; it adds no embedded language or change to `due run`. Use a
source-built Due executable with the optional `rpc` feature.

```sh
cargo build --locked --release -p due-cli --features rpc
node examples/session-harness/compare.mjs \
  --due ./target/release/due --provider codex --cwd /path/to/project \
  --artifacts /path/to/fresh-comparison --backend all
```

Select `--provider claude` to exercise Claude, or `--backend resume` or
`--backend rpc` for one arm. Both `--backend all` and the default `both` run
resume and RPC. Optionally select
`--model` / `--effort`. Running the comparison invokes real models. The default workflow requests no tools: it
asks for a small order total, then corrects the shipping fee. The second prompt
omits the marker and order details, testing conversation continuity. The caller
checks each JSON answer separately from execution success. A mismatch stops the
comparison with saved evidence; there is no automatic retry or fallback.

The script-facing API is small:

```js
import { withSession } from './session.mjs';

const output = await withSession({
  backend: 'rpc', // Or 'resume'; the workflow stays the same.
  dueBinary: './target/release/due',
  profile: { provider: 'codex', cwd: '/path/to/project', timeout: '2m' },
  artifactDir: '/path/to/fresh-session',
  maxTurns: 2,
  onItem: record => { console.log(JSON.stringify(record)); },
}, async agent => {
  const draft = await agent.turn({ id: 'draft', prompt: 'Prepare a draft.' });
  if (draft.status !== 'succeeded') return draft;
  return agent.turn({ id: 'correction', prompt: 'Revise the draft using these corrections...' });
});
```

`turn` returns the existing Due result shape. A failed native execution remains
data; transport/protocol failures throw. The callback owns one session, admits
one turn at a time, requires unique turn IDs, and must await all work. Returning
with active work fails and initiates cleanup. A handle retained after scope exit
rejects new turns. `maxTurns` is 1 through 64, default 4. This is a sequential
conversation primitive, not a general workflow scheduler or security sandbox.

The caller owns `profile`. The shared subset is `provider`, `cwd`, `model`,
`effort`, and `timeout`. The resume backend keeps that subset and enables
persistence for CLI resume. RPC additionally accepts `request`, `binary`,
`resume`, `ephemeral` and the supported `native_args`. For an existing Due JSON
profile, use `profile: {request: '/caller-owned/worker.json', cwd: '/path/to/project'}`.
The provider may come entirely from that file.

Rust owns strict file parsing and native-policy validation; the Node client does
not load or loosely project the JSON itself. Omitted options inherit file values,
explicit false/null overrides are preserved, and RPC does not force persistence
over a profile's `ephemeral` choice. Unsupported fields/options fail explicitly.
See the [profile rules and supported native options](../../docs/rpc.md#reusing-json-profiles).
`session.json` retains both the supplied `profile` and the server's
`resolved_options` (also in `session-open.json`); these are requested options,
not an audit of inherited native settings.

The [supervised assignment example](SUPERVISED.md) composes this API into
implementation, independent verification, checkpoints and bounded revision.

### Caller turn waits

For `rpc`, top-level `turnWaitMs` controls how long the client
waits for a terminal item after the turn-start response. Omit it to retain the
five-minute default (`300000`), pass an integer from 1 through 2147483647 for a
finite wait in milliseconds, or pass `null` for an unlimited wait. Use `null`
for work you actively monitor, with an `AbortSignal` to cancel the session.
Invalid values and explicit use with `resume` are rejected before artifacts or
processes are created. Persistent receipts record the effective `turn_wait_ms`.

This is a client wait, separate from the native per-turn `profile.timeout`.
It includes time spent waiting for a permission decision. `turnWaitMs: null`
does not disable native deadlines, the five-minute control-request waits, or
bounded shutdown. A finite client wait expiring fails the scope and starts
cleanup; it does not automatically retry or resume. This option is not sent to
the provider and does not change `due run`.

An unlimited wait relies on active monitoring and cancellation if a healthy
native turn stays running. Native or transport failures, and a native
`profile.timeout`, can still settle the turn. A finite `turnWaitMs` is not an
interrupt-and-continue operation: when it expires, the scope ends and waits for
cleanup. The client does not restart or resume that session automatically.

A `session_failed` item (native exit between or during turns) is kept in the
transcript, then fails the scope with its message. An earlier succeeded turn
stays succeeded; later turns are rejected locally without sending `turn.start`.
Incoming frames are decoded as strict UTF-8; invalid bytes fail the session.

The RPC backend writes `stdout.bin` (mode 0600, exclusive create): the exact bytes
Due wrote to stdout, captured before UTF-8 decoding or JSON parsing, including the
bytes of a malformed frame. It is limited to 64 MiB; exceeding the limit fails the
session and the file keeps the first 64 MiB. It is the authoritative wire evidence
for what Due sent, not proof of the native provider's complete history. Callbacks,
`items.jsonl` and `protocol.jsonl` are a JavaScript convenience view built with
`JSON.parse`: integers outside Number's safe range may be rounded (for example 9007199254740993
becomes 9007199254740992). Read `stdout.bin` when exact numbers matter.

A monitored two-turn Claude conversation in one scope, with
`onItem` receiving every full observed item (also saved in `items.jsonl`):
save this as an `.mjs` file beside `session.mjs`, replace the project/artifact
paths, and run it from the repository root.

```js
import { withSession } from './session.mjs';

const controller = new AbortController();
const stop = () => controller.abort();
process.once('SIGINT', stop);
try {
  const results = await withSession({
    backend: 'rpc',
    dueBinary: './target/release/due',
    profile: { provider: 'claude', cwd: '/path/to/project' },
    artifactDir: '/path/to/fresh-session',
    maxTurns: 2,
    turnWaitMs: null, // Unlimited client wait; the caller monitors and can abort.
    signal: controller.signal,
    onItem: record => { console.log(JSON.stringify(record)); },
    onInteraction: async (record, signal) => {
      signal.throwIfAborted();
      // Conservative: deny every approval request. This is not a sandbox and does
      // not restrict native tools that are already allowed.
      return { behavior: 'deny', message: 'Not approved by this caller.' };
    },
  }, async agent => {
    const first = await agent.turn({ id: 'first', prompt: 'Inspect the project and summarize it.' });
    if (first.status !== 'succeeded') return [first];
    const second = await agent.turn({ id: 'second', prompt: 'Now list the three riskiest files.' });
    return [first, second];
  });
  for (const result of results) {
    if (result.status !== 'succeeded') {
      throw new Error(result.error?.message ?? `Claude turn ${result.status}`);
    }
    console.error(`Claude turn ${result.status}`); // Keep stdout as JSON items.
  }
} finally {
  process.off('SIGINT', stop);
}
```

## What changes between backends

| | `resume` | `rpc` |
| --- | --- | --- |
| Invocation | A new `due run --request -` per turn | One `due rpc` process per scope |
| Conversation | Verified native ID carried as `resume` | One open native session |
| Observations | Existing normalized Due trace records | Full native session items exposed by the wrappers |
| Turn result | Result plus actual process receipt | Terminal result while process stays alive |
| Scope cleanup | Joins any active Due invocation | Closes session and drains output through process exit |

`onItem(record)` optionally observes records synchronously. Records are tagged
`source: 'due_trace'` or `source: 'native_session'`; they are not equivalent event
schemas or comparable event counts. The RPC backend recovers cursor gaps
with `session.read`, fails when retention is insufficient, and does
not use `turn.read` polling. Native permission requests fail explicitly unless the
caller supplies `onInteraction` (below).

### Interrupting and redirecting a worker

The RPC scope advertises `agent.capabilities.interrupt` and exposes an exact-turn
operation using the same caller ID passed to `turn`:

```js
const running = agent.turn({id: 'investigate', prompt: 'Investigate the failing test.'});
// After observing progress or receiving a new decision from the parent:
await agent.interrupt({id: 'investigate'});
const interrupted = await running;
const followup = await agent.turn({id: 'redirect', prompt: 'Focus on the parser instead.'});
```

Interruption acknowledgement and turn completion are separate; await both. An
interrupt admitted before the Due turn ID arrives waits for that exact ID. Late
acknowledgements never target a later turn. Stale IDs, duplicate attempts and a
second outstanding interruption are rejected locally. A remote rejection is
catchable without making the session unusable. The scope records control
outcomes and joins outstanding controls during cleanup. The resume backend
rejects this operation.

`agent.signal` is available to cooperative caller-owned checks and checkpoints.
It aborts on parent cancellation, native/transport failure and scope exit.
Graceful turn interruption leaves it usable for the follow-up. Caller callbacks
must observe it and bound/join their own work; the scope cannot preempt arbitrary
JavaScript or clean up detached verifier processes.

### Answering native permission requests

`withSession({ ..., onInteraction })` optionally takes `onInteraction(record, signal)`
for the `rpc` backend. `record` is a copy of the record `onItem`
saw (`{ source: 'native_session', turn_id, data }`), where `data` is the complete
`interaction` item. The callback may be async and returns the provider-native
response object, for example `{ decision: 'decline' }` for Codex command approval
or `{ behavior: 'deny', message: '...' }` for Claude. Nothing is translated and
nothing is approved by default.

- Absent callback: the existing fail-closed behavior. The turn fails with
  "requires an explicit decision" and receipts are kept.
- The reply is sent as `interaction.reply` using the
  `session_id`, `turn_id` and `data.request_id` Due sent, snapshotted before
  `onItem` runs, so an observer mutating its record cannot redirect a reply.
  `onItem` still observes the complete raw item synchronously, and stdout parsing
  and later items continue while the decision is pending.
- While the decision or its reply is pending, `signal` aborts when the interaction
  resolves, the turn finishes, the session fails or is cancelled, or the scope
  closes. A result returned after that is discarded and never sent. Close does
  not wait for the callback. Rejecting with `signal.reason` (or an
  `AbortError`) after an abort is treated as cooperative cancellation.
  Once the callback and reply settle, their signal is no longer tracked.
- A thrown/rejected callback or a non-object response fails the session with
  that error; process receipts are preserved. The same applies to a callback
  error that arrives after the turn finished but before scope settlement.
- A reply Due rejects as an unknown/finished turn or resolved interaction is
  ignored only when that interaction's resolution or its turn's terminal item
  was observed. Any other rejection observed before scope settlement, including
  one arriving after the terminal item, is a session failure.
- An `interaction_resolved` item aborts the pending decision for that exact
  session, turn and request ID only; resolutions of other requests do not. A
  result returned by the resolved decision is discarded without sending a reply.
- A reply failure that arrives while scope close is pending is the body failure
  (`body_error`). The close call it collaterally rejects is not reported as
  `cleanup_error`; that field stays reserved for the session close itself failing
  or the Due process exiting non-zero.
- Close does not wait for a callback that ignores its signal. A callback that
  rejects after the scope has closed is handled (no unhandled rejection) and its
  reply is never sent, but it cannot fail the completed scope or rewrite the saved
  `session.json`. Only failures observed before close completes are recorded.
- `agent.capabilities.interactionReply` is true only for `rpc` with a
  callback. `resume` with `onInteraction` is rejected before any artifact
  directory is created or process launched, as is a non-function value.

For the RPC backend, a fresh Claude session can open with
`native_session_id: null`. The ID is then taken from the root
`system`/`init` event of a `native_summary` or `native_snapshot` item (provider
`claude`) before observers run, so it is kept in the receipt even if an observer
or the protocol fails afterwards. IDs in nested, non-init, assistant or non-Claude
payloads are ignored, and a later conflicting root init fails the session.

Native IDs must stay stable within a scope. The resume adapter keeps an available
ID after an explicit failed turn, so a deliberate follow-up can use it; if the
first attempt supplies none, it refuses to silently start a fresh conversation.
Malformed transport evidence makes the session unavailable.

## Evidence and limits

Every session reserves a fresh artifact directory. `session.json` records turns,
control operations, requested/resolved options, native identity, open/close
durations, process receipts and separate body/cleanup errors. `items.jsonl`
saves observed records as they arrive, with a 64 MiB limit.
Per-turn directories preserve requests and results. Resume also captures each
process's raw stdout, stderr and exit; resident saves its protocol JSONL,
diagnostics, open response and final process receipt. Files use mode 0600 and
directories 0700 subject to umask. They contain prompts and native payloads.

The resume adapter parses at most 8 MiB of result JSON. Its cooperative cleanup
waits for Due process closure, as the existing Node caller does. Resident frames
are capped at 16 MiB, its pending notifications at 1024 / 8 MiB, and protocol
evidence at 64 MiB. Resident control RPC waits have a five-minute client deadline;
terminal-result waits use `turnWaitMs` (five minutes by default). The native
profile deadline is separate. Resident close has bounded waits and
can kill the Due process if it remains stuck, reporting that failure. Neither
backend promises cleanup of every escaped native descendant. These differing
cleanup policies are explicit experimental limits, not backend parity claims.

`comparison.json` retains each completed arm and any comparison failure. Timing
is a single end-to-end observation, including model/network variability. It is
not a startup benchmark or evidence that one transport improves answer quality.
All arms use the same requested profile and workflow, but effective native
configuration should be inspected independently where the providers expose it.

## Plain JSON-RPC

`resident.mjs` owns the process, streaming, cursor recovery and cleanup. It starts
`due rpc` and calls `session.open`, `turn.start`, `turn.interrupt`, `session.read`,
`interaction.reply`, and `session.close` directly. It consumes `session.item`
notifications, validates `protocol_version: 1` in the open result, and requires
the advertised stream.

The client needs no discovery or SDK. Native turn failure is result data;
transport and RPC errors throw. Native permission replies are exposed through
`onInteraction`; `agent.interrupt({id})` exposes a graceful mid-turn stop without
closing the session.

For a small two-turn RPC client that writes every observed native item to stdout:

```sh
cargo build --locked --release -p due-cli --features rpc
node examples/session-harness/rpc-example.mjs \
  --due ./target/release/due --provider codex --cwd /path/to/project \
  --artifacts /path/to/fresh-rpc-session > /path/to/session-items.jsonl
```

The example runs `correctionWorkflow`, which makes two `agent.turn` calls and
independently validates both answers. Use `--provider claude` for Claude. Results
and process evidence are also retained under the fresh artifact directory.
[The RPC guide](../../docs/rpc.md) describes the wire methods and limitations.
Future embedded Lua or QuickJS bindings can call the shared Rust session host
directly; external transports do not have to become part of the script API.

## Offline checks

```sh
node --test examples/session-harness/*.test.mjs
```

The tests launch synthetic Due peers and use no credentials or models. They
exercise process/session continuity, early notifications, recovery, truthful
outcomes, cancellation, scope ownership and the shared correction workflow.

## Live RPC observation (2026-11-01)

The plain-RPC run completed the same correction with both providers: 43 Claude
items and 99 Codex items, no cursor gaps or push drops, stable native session
IDs, and clean Due exits. It used the RPC-only binary, four direct requests per
conversation, and no polling. See the
[RPC observation](../../docs/rpc.md#live-observation-2026-11-01) for versions and
limits. This verified conversation continuity and item delivery, not comparative
performance or answer quality.

The earlier resume/MCP comparison is preserved on the separate
[MCP spike branch](https://github.com/joshrotenberg/due/tree/codex/due-mcp-spike/examples/session-harness).
