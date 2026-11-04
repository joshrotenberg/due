# Core library spike

Experimental implementation of the five-function design in
[PR #147](https://github.com/joshrotenberg/due/pull/147). This branch implements
the library first. It does not introduce an MCP adapter or JS client, remove
`due rpc`/`due script`, or replace installed guidance/plugins.

## Library boundary

The existing `due` package now also builds a Rust library. Enable `core` without
the `rpc` or `harness` features to use it without a wire server or interpreter:

```sh
cargo test --locked -p due --no-default-features --features core --test core
```

The spike reuses the pinned Claude `DuplexSession` and Codex `AppServer` backends
directly. Each logical worker owns a native resident transport; an engine owns
N workers. It does not invoke a child `due rpc` or synthesize CLI arguments for
Due. Provider types, results, duration parsing and resident policy validation
are shared Rust code outside Clap parsing. The package still contains the CLI
dependencies; this is a library target, not a separately published core crate.

The old RPC adapter temporarily uses a hidden single-session compatibility
module. New library callers use `core::Engine`. Transport/profile-file adapters
remain separate from core execution; no MCP tool/schema contract is implied.

## Interface

| Function | Actual spike API |
| --- | --- |
| `spawn` | `engine.spawn(Assignment { worker, prompt, timeout }).await` |
| `follow_up` | `engine.follow_up(SessionRef or SavedSession, FollowUp { prompt, timeout }).await` |
| `inspect` | `engine.inspect(&run)`; synchronous, non-consuming |
| `wait` | `engine.wait(&runs, WaitOptions { until, timeout }).await` |
| `interrupt` | `engine.interrupt(&run).await`; targets the exact assignment and awaits terminal settlement |

`WorkerOptions` fixes provider, canonical cwd, resolved executable, requested
model/effort, ephemeral choice and audited native policy arguments at admission.
Construct it with `WorkerOptions::new(provider, cwd)`, then set explicit fields.
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
unchanged. A shutdown wait expiring does not invent successful settlement, and
the owner continues cleanup. Dropping a shutdown future also leaves its independent
cleanup owner running. Run snapshots remain readable after shutdown.
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

The initial spike has no permission-response, fork, steer or full-item client API.
Required native interactions fail explicitly with `unsupported` and close that
worker; they are never auto-approved or left pending indefinitely. Native policy
options still apply. These capabilities need deliberate API work before MCP can
offer them. Fresh/resume input uses the existing resident policy allowlist, not
the wider one-shot passthrough surface; answer schemas are also not added here.

Darwin native opens are serialized across cooperating engine instances until
wrapper initialization completes, avoiding concurrent core-owned pipe setup.
Active turns execute concurrently. This boundary does not protect unrelated
process spawns by an embedding application and does not claim to solve every
historical #19 case. The paused harness fix is not pulled into this branch.

## Verification

`tests/core.rs` uses the existing resident protocol fixture in independently
configured temporary directories. It launches fake native executables directly,
without Due RPC, credentials, permission bypasses or a real model. Tests cover
both providers' fresh/live/saved continuity, identity, ephemeral behavior,
concurrency, result retention, wait semantics, stale/duplicate interruption,
deadline and startup races, pending/rejected control acknowledgements, unsupported
interactions, capacity/validation, sibling isolation, native failure between
turns and confirmed PID reaping. Cleanup failures remain separate and sticky.

The existing CLI/RPC/harness suites remain regression coverage. CI adds the
core-only build/test combination on Linux and macOS and a Rust 1.90 core-only
check. Local fake-provider success is not evidence of every real native version,
approval policy or platform. No dependencies or wrapper revisions are upgraded.
