# Core library spike

Experimental implementation of the five-function design in
[PR #147](https://github.com/joshrotenberg/due/pull/147). This branch implements
the library first. It does not introduce an MCP adapter or JS client, remove
`due rpc`/`due script`, or replace installed guidance/plugins.

## Library boundary

Use either provider on its own, or select a provider for each worker in a
mixed workflow. Install and authenticate the native CLI for the providers
you use. The engine shares run and session operations across providers.
Native tool configuration and approval responses remain provider-specific.

`due-core` is a separate workspace crate. Its default `resident` feature
provides `due_core::Engine` and the native Claude `DuplexSession` and Codex
`AppServer` backends. Each worker owns one resident native transport, and
an engine can supervise several workers. The crate has no dependency on
the Due CLI, its wire server, or a scripting interpreter.

The `due-cli` package builds the `due` executable. Its foreground command
uses `due_core::oneshot::execute` for native option validation, process
execution, and result normalization. The CLI resolves its inputs, renders
the result, and translates observer callbacks into its trace format. Its
`core` feature enables `due-core/resident`, and `rpc` and `harness` build on
that feature. The current single-session RPC command uses the hidden
compatibility export. Profiles, command parsing, and wire schemas live
in the application.

The [comparison example](../examples/core-compare) is a second executable
that depends on `due-core` and Tokio. It runs the same prompt in independent
Claude and Codex workers, sends a follow-up to both, and reads each session's
journal after cleanup.

```sh
cargo test --locked -p due-core -p due-core-compare
cargo run --locked -p due-core-compare -- 'Describe this repository in two sentences.'
```

The test uses fake native executables. The `cargo run` example starts the
real provider CLIs using the caller's native settings. For explicit
executables, set `COMPARE_CLAUDE_BINARY` and `COMPARE_CODEX_BINARY`. The
example rejects interactions and gives each turn a two-minute budget.

The second consumer uses the same `RunResult` as the CLI. Fields for CLI
invocation, tracing and schema validation remain on that shared type. A
future API revision can decide how much of that data belongs in the core
result. This spike tests the package boundary while preserving the result
contract.

## Try a separate author and reviewer

Run the [offline walkthrough](../examples/core-compare/tests/author_reviewer.rs)
from the repository root:

```sh
cargo test --locked -p due-core-compare --test author_reviewer -- --nocapture
```

It starts a fresh author and reviewer using fake Claude and Codex executables,
sends one follow-up to each original session, and checks that both native
processes exit. The diff and review feedback are sample inputs. The fixture
answers count turns, so the example checks session continuity and cleanup.
For the lifecycle operations available on other surfaces, see
`due skills show use-cases`.

## Interface

| Function | Actual spike API |
| --- | --- |
| `spawn` | `engine.spawn(Assignment { worker, prompt, timeout }).await` |
| `follow_up` | `engine.follow_up(SessionRef or SavedSession, FollowUp { prompt, timeout }).await` |
| `fork` | `engine.fork(saved, FollowUp { prompt, timeout }).await`; branch persisted native history into a new worker |
| `inspect` | `engine.inspect(&run)`; synchronous, non-consuming |
| `wait` | `engine.wait(&runs, WaitOptions { until, timeout }).await` |
| `interrupt` | `engine.interrupt(&run).await`; targets the exact assignment and awaits terminal settlement |
| `respond` | `engine.respond(&interaction.id, native_response).await`; answers one pending generation |
| `subscribe` | `engine.subscribe(&session, after_cursor)`; independent full-item reader across turns, with `next().await` or `read_available(limit)` |

`WorkerOptions` fixes provider, canonical cwd, resolved executable, requested
model/effort, ephemeral choice and audited native policy arguments at admission.
Construct it with `WorkerOptions::new(provider, cwd)`, then set explicit fields.
Use `WorkerOptions::validate()` to resolve paths and check the native policy
before admitting a worker. It returns the resolved options without starting
the provider. Admission also validates options supplied directly.
There are no role, scheduling, verification or automatic retry decisions.

Run and session references are distinct opaque IDs. Passing a handle to another
engine fails. Snapshots retain an `Arc<RunResult>` independently of subsequent
turns; repeated inspection does not consume the result. The result's `run_id`
is the engine run ID; `session_id` remains the observed native identity.

Native context has two paths: a live `SessionRef` reuses that native process;
a `SavedSession` imports persisted native history into a new resident process.
Saved descriptors freeze explicit requested configuration, not inherited grants,
credentials or native filesystem contents. Resume identity mismatches fail.
An ephemeral worker supports live follow-ups but publishes no resume descriptor.

Use `engine.fork(saved, FollowUp::new("Explore a different approach"))` when a
new worker should start with the source's persisted context. Use an idle source
with saved history. The core reuses its explicit worker configuration and gives
the branch a new logical session reference. Due waits for a distinct native
identity before publishing a resume descriptor. With Claude, the provider may
first report that identity during the assignment. With Codex, the fork response
reports the new thread before the assignment starts. The session journal records
the requested source as `session_opened.resolved_options.fork_source` and retains
Codex's native response, including `forkedFromId` when present. A provider that
rejects the operation yields a failed run with its diagnostic.

A fork copies conversation context. Workspace files and side effects are shared
when both workers use the same workspace. For independent review, start a fresh
worker and pass the issue, diff, and verification evidence. The core API owns
forking in this slice. The foreground CLI, RPC and JavaScript adapters retain
their existing session operations.

`wait` returns snapshots in requested order. `Any` includes already-completed
runs; `All` requires every requested run to finish. A zero wait budget polls;
finite expiry returns `condition_met: false`. Dropping a wait future leaves work
running. Empty/duplicate run sets, foreign IDs and clock-overflow durations fail.

The assignment deadline includes time after admission, including startup queue
time. Expiry requests interruption and awaits cleanup; it is not a hard real-time
return guarantee. A native startup/control operation already in progress is
allowed to settle under the wrapper's own bounds. Interruption/shutdown can
report settlement pending while that happens. Claude may queue a prompt before
native initialization finishes. Once the owner observes a stop during startup,
it sends no turn.

## Ownership, failures and bounds

Call `engine.shutdown().await` before releasing its Tokio runtime. Shutdown
rejects new work, cancels active runs and closes every native worker. Repeated
calls preserve cleanup failures; earlier successful assignment results remain
unchanged. If shutdown returns a settlement error while runs are still active,
keep the runtime alive and retry once the runs have settled so idle workers
can close too. Dropping a polled shutdown future leaves its independent
cleanup owner running through its configured bounds. Run snapshots remain
readable after shutdown.
Drop requests best-effort cleanup on the captured runtime; it cannot report
success and is not a substitute for awaited shutdown.

Defaults retain at most 64 admitted runs and 16 logical workers, with a
90-second settlement budget per cleanup/wait stage. Limits are caller supplied.
There is no eviction, persistent engine registry or automatic slot recycling;
capacity exhaustion is explicit. Use an engine scope appropriate to the task.
Native transport output/event limits remain enforced by the reused backends.
Prompts and native argument bytes are bounded to 1 MiB; native arguments also
have a 4096-element limit. Model input is bounded to 1 MiB. No model calls occur
in the test suite.

Execution failures are result data. Invalid input, busy/foreign handles, exhausted
capacity, closed engines and unconfirmed operation settlement are typed operation
errors. A worker failure does not automatically cancel siblings. A native session
that failed or could not be settled rejects further live follow-ups. Ordinary
failed turns can be followed by a later turn in a healthy worker.
Native control/transport failures after admission are retained as protocol
failures on the run. `interrupt` can therefore return a finished, failed snapshot;
an `Ok` return alone does not mean successful interruption. Inspect its result.

## Approvals and other native interactions

`WorkerOptions::new` defaults to `InteractionMode::Manual`. The core never
chooses an approval policy. A pending request appears in `inspect().interactions`
and as a streamed `interaction` item, with the full native request and an opaque
`InteractionRef`. Pass that token and a provider-native JSON object to `respond`.
Tokens identify the exact engine run, native request and journal generation;
reused native request IDs cannot redirect an old response to a new request.
Multiple outstanding requests can be answered independently. Duplicate in-flight
responses return `Busy`; resolved/finished/retired tokens return
`UnknownInteraction`. Foreign run tokens return `UnknownRun`.

Claude responses are `{"behavior":"allow"}`, optionally with an object
`updated_input`, or `{"behavior":"deny","message":"reason"}`. Codex response
objects follow the request's native method, for example `{"decision":"accept"}`
or `{"decision":"decline"}` for command approval. Codex tool questions and other
request types need their own native response shape; core does not invent one
normalized approval schema. Invalid local inputs leave the request pending.
Responses are bounded to 1 MiB.

A `ResponseReceipt` confirms wrapper acceptance for delivery, not native execution
or completion. Observe later items and the terminal run result. Dropping the
response future after submission leaves an independent owner responsible for
settlement. Run completion waits for submitted responses to settle. Delivery
failure, timeout or panic is retained as a protocol failure and closes that
worker; delivery may be unconfirmed and must not be automatically retried.
A request resolved before dispatch is a stale operation, not a new run failure.

Waiting does not auto-answer requests. In manual mode an assignment without an
execution deadline can wait indefinitely for input. Its assignment deadline,
`interrupt` and engine shutdown still apply. Unattended callers can instead set
`interaction_mode = InteractionMode::Reject`; encountering an interaction then
fails the run explicitly with `unsupported` and closes that worker. Neither mode
changes native permission grants. Live steering remains outside this spike.
Fresh/resume input uses the existing resident policy allowlist, not the wider
one-shot passthrough surface; answer schemas are also not added here.

## Full session item stream

`subscribe(&session, 0)` replays retained items and follows new ones, including
startup, native requests/responses/notifications, partial chunks, interaction
resolution and turn/session lifecycle items. Each `SessionItem` has a logical
session, optional core run, monotonic session cursor, kind, full JSON data and
an optional reply token. Unknown nested native fields are preserved. Native
session/thread IDs stay inside native payloads. `run_finished` contains the
authoritative core result after control settlement; native `turn_finished` may
precede that barrier.

This is parsed wrapper evidence, not byte-for-byte native traffic. The pinned
Claude wrapper reduces live init to an identity summary; complete init is
recovered as a `native_snapshot` after a successful turn when available. Wrapper
subscriber loss is exposed through native `events_lost` summaries. Native policy
still determines which tool/provider events are emitted. Prompts, tool inputs,
outputs and responses can be present: this is not a redacted diagnostic trace.

Readers have independent, cancellation-safe cursors. Dropping a stream never
cancels a run; readers do not apply backpressure to providers. A stream remains
open while its worker is idle between turns. Save `stream.cursor()` and subscribe
again from that exclusive cursor. Retained history is readable after shutdown.
`None` means a closure marker has been delivered and retained items are drained,
not merely that the current turn ended.

For a nonblocking read, call `read_available(limit)` on the same reader. It
returns up to 256 items within a 4 MiB page budget, an exclusive next cursor,
the latest cursor at the time of reading, and whether the journal is closed. An empty page from an
active session means it has caught up for now. Continue reading after closure
until the page cursor reaches the latest cursor. Both read methods report
retention gaps without advancing the reader.

The shared journal retains at most 1024 items / 8 MiB per worker. An individual
item over 4 MiB is replaced by an explicit `omitted` marker at the same cursor.
If an interaction item is omitted, its pending request/token remains available
through `inspect`. A closure marker ends this journal; its error can still report
unconfirmed native cleanup. If history is evicted before a reader consumes it, `next()` returns
`StreamError::Lagged { oldest_available, .. }` without advancing its cursor;
resubscribe from `oldest_available - 1` to explicitly accept that gap. Cursors
from another session are not portable; a cursor ahead of the producer is rejected.
This is bounded in-memory replay, not a durable audit log.

A conservative Claude caller can stream and decline any request it has not
chosen to grant:

```rust,no_run
use due_core::*;
use serde_json::json;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let engine = Engine::new(Limits::default())?;
let worker = WorkerOptions::new(Provider::Claude, std::env::current_dir()?);
let run = engine.spawn(Assignment::new(worker, "Review the parser.")).await?;
let session = engine.inspect(&run)?.session;
let mut items = engine.subscribe(&session, 0)?;
while let Some(item) = items.next().await? {
    if let Some(token) = &item.interaction {
        match engine.respond(token, json!({"behavior":"deny","message":"Read-only review"})).await {
            Ok(_) => {},
            Err(error) if error.kind == ErrorKind::UnknownInteraction => {},
            Err(error) => return Err(error.into()),
        }
    }
    println!("{}: {}", item.kind, item.data);
    if item.kind == "run_finished" { break; }
}
let next = engine.follow_up(session, FollowUp::new("Summarize the findings.")).await?;
// Consume subsequent stream items and handle interactions before waiting forever.
engine.interrupt(&next).await?;
engine.shutdown().await?;
# Ok(())
# }
```

Darwin native opens are serialized across cooperating engine instances until
wrapper initialization completes, avoiding concurrent core-owned pipe setup.
Active turns execute concurrently. This boundary does not protect unrelated
process spawns by an embedding application and does not claim to solve every
historical #19 case. The paused harness fix is not pulled into this branch.

## Verification

`crates/due-core/tests/core.rs` uses the existing resident protocol fixture in independently
configured temporary directories. It launches fake native executables directly,
without Due RPC, credentials, permission bypasses or a real model. Tests cover
both providers' fresh/live/saved continuity, identity, ephemeral behavior,
concurrency, result retention, wait semantics, stale/duplicate interruption,
deadline and startup races, pending/rejected control acknowledgements, manual approval/denial, stale and
reused native request IDs, dropped/duplicate response futures, independent full-item
readers, replay/retention/closure, explicit reject mode, capacity/validation, sibling isolation, native failure between
turns and confirmed PID reaping. Cleanup failures remain separate and sticky.

The existing CLI/RPC/harness suites remain regression coverage. CI adds the
core-only build/test combination on Linux and macOS and a Rust 1.90 core-only
check. Local fake-provider success is not evidence of every real native version,
approval policy or platform. No dependencies or wrapper revisions are upgraded.
