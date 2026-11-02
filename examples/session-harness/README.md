# A session API over CLI resume and resident execution

This optional Node.js 22+ experiment runs the same JavaScript correction workflow
through CLI resume, resident MCP, or resident plain JSON-RPC. The JavaScript uses
Node built-ins only; it adds no embedded language or change to `due run`. Use a
source-built Due executable with the selected optional transport feature.

```sh
cargo build --locked --release --features mcp,rpc
node examples/session-harness/compare.mjs \
  --due ./target/release/due --provider codex --cwd /path/to/project \
  --artifacts /path/to/fresh-comparison --backend all
```

Select `--provider claude` to exercise Claude, or `--backend resume`,
`--backend resident` (MCP), or `--backend rpc` for one arm. `--backend all` runs all
three; the existing default `both` still runs resume and MCP. Optionally select
`--model` / `--effort`. Running the comparison invokes real models. The default workflow requests no tools: it
asks for a small order total, then corrects the shipping fee. The second prompt
omits the marker and order details, testing conversation continuity. The caller
checks each JSON answer separately from execution success. A mismatch stops the
comparison with saved evidence; there is no automatic retry or fallback.

The script-facing API is small:

```js
import { withSession } from './session.mjs';

const output = await withSession({
  backend: 'rpc', // Or 'resume' or 'resident'; the workflow stays the same.
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
`effort`, and `timeout`. Unknown fields are rejected before launch. In particular,
this experiment cannot silently import the older harness's `native_args`,
schema-validation or sandbox profiles into the resident backend. Authentication
and native permission policies are inherited; no permission bypass or global
configuration change is added. Persistence is enabled to allow CLI resume.

## What changes between backends

| | `resume` | `resident` | `rpc` |
| --- | --- | --- | --- |
| Invocation | A new `due run --request -` per turn | One `due mcp` process per scope | One `due rpc` process per scope |
| Conversation | Verified native ID carried as `resume` | One open native session | One open native session |
| Observations | Existing normalized Due trace records | Full native session items exposed by the wrappers | Same native session items |
| Turn result | Result plus actual process receipt | Terminal result while process stays alive | Same |
| Scope cleanup | Joins any active Due invocation | Closes session and drains output through process exit | Same |

`onItem(record)` optionally observes records synchronously. Records are tagged
`source: 'due_trace'` or `source: 'native_session'`; they are not equivalent event
schemas or comparable event counts. Both persistent backends recover cursor gaps
with `session_read` / `session.read`, fail when retention is insufficient, and do
not use `turn_read` polling. Native permission requests fail explicitly in this small
comparison; no approval callback or mid-turn interruption API is exposed yet.

Native IDs must stay stable within a scope. The resume adapter keeps an available
ID after an explicit failed turn, so a deliberate follow-up can use it; if the
first attempt supplies none, it refuses to silently start a fresh conversation.
Malformed transport evidence makes the session unavailable.

## Evidence and limits

Every session reserves a fresh artifact directory. `session.json` records turns,
native identity, open/close durations, process receipts and separate body/cleanup
errors. `items.jsonl` saves observed records as they arrive, with a 64 MiB limit.
Per-turn directories preserve requests and results. Resume also captures each
process's raw stdout, stderr and exit; resident saves its protocol JSONL,
diagnostics, open response and final process receipt. Files use mode 0600 and
directories 0700 subject to umask. They contain prompts and native payloads.

The resume adapter parses at most 8 MiB of result JSON. Its cooperative cleanup
waits for Due process closure, as the existing Node caller does. Resident frames
are capped at 16 MiB, its pending notifications at 1024 / 8 MiB, and protocol
evidence at 64 MiB. Resident RPC/turn waits have a five-minute client deadline;
the native profile deadline is separate. Resident close has bounded waits and
can kill the Due process if it remains stuck, reporting that failure. Neither
backend promises cleanup of every escaped native descendant. These differing
cleanup policies are explicit experimental limits, not backend parity claims.

`comparison.json` retains each completed arm and any comparison failure. Timing
is a single end-to-end observation, including model/network variability. It is
not a startup benchmark or evidence that one transport improves answer quality.
All arms use the same requested profile and workflow, but effective native
configuration should be inspected independently where the providers expose it.

## MCP versus plain JSON-RPC

`resident.mjs` shares process ownership, streaming, cursor recovery and cleanup
between the two persistent transports. The RPC variant starts `due rpc` and
calls `session.open`, `turn.start`, `session.read`, and `session.close` directly.
It consumes `session.item` notifications. It validates `protocol_version: 1` in
the open result and requires the advertised stream. MCP initializes the protocol
and calls the same operations through `tools/call` with structured tool results.

The RPC client needs no discovery or MCP SDK. It removes the initialization,
tool envelopes and result-content wrapper; it retains the same lifecycle work.
Both variants distinguish native turn failure (result data) from a transport or
RPC error (an exception). Neither variant in this small client exposes permission
replies or mid-turn interrupts, even though both Due server adapters support them.

For a small two-turn RPC client that writes every observed native item to stdout:

```sh
cargo build --locked --release --features rpc
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

## Earlier resume/MCP live comparison (2026-11-01)

The earlier comparison below predates the RPC adapter. On macOS arm64 / Node
26.12.1, Claude Code 2.1.316 and Codex CLI 0.165.0 each completed the identical two-turn correction through both backends. Requested
profiles used `sonnet` / `gpt-6-astra` respectively, low effort, a two-minute
native deadline, and the same disposable workspace. Both remembered the marker
and original order details, producing totals 19 then 17. No tools were requested.

| Provider | Backend | Due processes | Observed total time | Observation records |
| --- | --- | ---: | ---: | ---: |
| Claude | CLI resume | 2 | 11.71 s | 20 Due trace records |
| Claude | Resident MCP | 1 | 6.69 s | 43 native session items |
| Codex | CLI resume | 2 | 11.78 s | 19 Due trace records |
| Codex | Resident MCP | 1 | 7.16 s | 103 native session items |

Every arm preserved one native session ID across its two turns. All six Due
processes exited 0. Resident captures had consecutive cursors, zero reported
push drops, two terminal results and the final session-closed item. The temporary
workspace remained unchanged. The observed record types differ by backend.

These are single observations, with resume first within each provider; the two
provider comparisons overlapped. They do not isolate startup overhead, caching,
model latency or network variation and are not a performance ranking. Requested
settings matched within each provider; this does not prove identical effective
configuration across its native transports. Permissions remained inherited.

The experiment used the existing MCP-feature release binary at source base
`a303cc89a6df810243125a6e8c3f2632eb29d32d` plus the uncommitted MCP changes.
Forty-three offline tests passed, including the identical application function
on both synthetic backends. No Rust runtime or wrapper revision changed for
this JavaScript experiment. The useful conclusion is API portability and
conversation continuity; richer live controls remain a separate follow-up.


The subsequent plain-RPC run completed the same correction with both providers:
43 Claude items and 99 Codex items, no cursor gaps or push drops, stable native
session IDs, and clean Due exits. It used the RPC-only binary, four direct
requests per conversation, and no polling. See the
[RPC observation](../../docs/rpc.md#live-observation-2026-11-01) for versions and
limits. The existing timing table above remains the earlier resume/MCP sample;
it is not a timing comparison against RPC.
