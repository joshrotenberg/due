# Integrated module and workflow spike

This is a Unix, process-scoped CLI and library inside the isolated spike crate.
It uses the **same** pinned `Compiler`, immutable `data` userdata and
`lua/contract.lua` as the language probes. The integrated tests execute actual
Fennel modules, including `defn` wrappers that yield through Rust async calls.
The native child is a synthetic Due-compatible executable, **not live provider
evidence**. Root Due is binary-only, remains unchanged, and is not a Rust library
dependency. The installed Due binary also remains unchanged.

## Invocation

Build offline with the selected Rust 1.90 toolchain:

```sh
FINOCCHIO_TOOLCHAIN_BIN=/path/to/toolchain/bin sh spikes/finocchio/test.sh
```

The executable is `target/finocchio/debug/finocchio-spike`. The new commands are:

```text
finocchio-spike workflow describe CONFIG.json
finocchio-spike workflow check CONFIG.json
finocchio-spike workflow run CONFIG.json INPUT.json FRESH_ARTIFACT_DIRECTORY
```

For a zero-agent run, use `examples/workflow/pure.json` and `input.json`. Supply
absolute command arguments when running from another directory. Tests actually
run the CLI with `/` as cwd, while keeping all writes under checkout `target/`.
No provider binaries, Due binary, profiles or roles are needed for a pure program.
`max_jobs` defaults to zero. The existing compile/semantic commands remain.

`Config::load` resolves paths relative to the config file. A library-constructed
`Config` must contain absolute paths; execution canonicalizes them. `modules`
is the entire allowed runtime module map; no ambient package paths, directory
search, native extensions or dynamic file loading exist. Each `.fnl` file returns
an ordinary export table. Cycles and missing modules fail. All configured modules
are compiled and initialized, even if the entry does not import them. `require`
returns cached exports thereafter. Macro search remains the fixed in-memory
Finocchio macro library, with the separate compiler VM's existing constraints.

Both `check` and `describe` load the definitions and inspect `meta`, without
calling exported `run`, `prepare` or `validate` functions. `check` is intentionally
this narrow executable loading/contract check, **not** general static checking
or the older semantic tool. Initializers are ordinary code and may call pure
functions, but cannot admit jobs or concurrency, even under `pcall`. Calling an
example agent's `run` during initialization is denied. Compilation itself may
execute macros, which also have no agent capabilities. Pure initializer errors
and attempted effects are sticky. All configured modules must load successfully.

Output is one JSON envelope with `status`, `error`, `output`, and `steps`.
Workflow errors include a category/message and generated-Lua filenames where
available. Compiler errors retain Fennel filenames/locations. Generated Lua line
numbers are explicitly labelled; they are not claimed as Fennel callsite spans
or arbitrary macro provenance. Setup IO/config errors are also JSON. Exit codes
are 0 for successful program evaluation, 1 for failure, 130 for parent signal.
A domain `attempts_exhausted` or a handled job failure may be a successful
*program evaluation*: inspect `output.status` and individual `steps[].status`.

## Ordinary modules, metadata and functions

`reviewer.fnl` exports immutable `meta` and ordinary `prepare`, `validate`, `run`
functions. It is used directly as the entry and unchanged inside `review-cycle`
and `parallel`. `reviser.fnl` follows the same convention. No object system,
agent registry, compiler fork, scheduler daemon or provider protocol is added.

Metadata fields:

| Field | Meaning and enforcement |
| --- | --- |
| `intent`, `skills` | Description. Examples include intent in their prepared prompt. A skill label **does not install, load, authenticate or enforce a skill**. Custom functions can prepare different prompts. |
| `model` | Optional `{exact:{provider,model,effort}}` or `{preference:"careful"}`. Rust selects the first matching profile from the caller's ordered role grant; there is no model catalog, implicit fallback or profile creation. Exact selection compares all three settings, including omitted effort. |
| `input_schema` | Enforced at the entry boundary and each job admission. The example functions also validate before preparation. |
| `output_schema` | Enforced on the entry's returned value. An agent module returns an **outcome**, distinct from a native answer. Arbitrary nested module calls remain ordinary functions, not automatic module contracts. |
| `answer_schema` | Required for a module bound to an agent role. Written to a host-owned schema file, sent to Due, and independently validated by Rust. Due's `answer_schema_validation: not_performed` is not treated as validation. |

Schemas use the documented no-reference Draft 2020-12 subset from Due's
experimental host: types, enum/const, required/properties/additionalProperties,
items, size and numeric constraints, uniqueItems, allOf/anyOf/oneOf/not, titles
and descriptions. `$ref`, remote resolution, formats, regex/pattern and unknown
keywords are rejected. Metadata is snapshotted before entry execution; assigning
a new `exports.meta` during a run cannot change the host's contracts or selection.

The host API deliberately has only three operations:

```fennel
(host.validate :reviewer :input input) ; or :output for the answer schema
(host.run {:role :reviewer :workspace :a :input input :prompt (prepare input)})
(host.concurrent [(fn [] (saved-workflow a)) (fn [] (saved-workflow b))])
```

`host.run` returns an immutable JSON outcome and owns the job until collection.
It accepts **only** those four request fields. It does not accept a profile name,
binary, provider, model, native arguments, cwd, schema path or permission change.
`host.concurrent` returns an immutable JSON array in input order, independently
of completion order. Every function must return exactly one JSON-compatible
value. Ordinary shared lexical state remains shared, so authors should avoid
using mutation to depend on completion order. The examples use immutable inputs.
Invalid host admission/concurrent arguments are sticky even if caught.

The ordinary `review-cycle.with-stages(input, review, revise)` function contains
all review policy. `max_revisions` is **the maximum number of mutation calls**
(integer 0..3), so there may be one initial review plus at most that many
subsequent reviews. Approval terminates immediately. A successful native review
with `changes_requested` is a domain result, not execution failure. Exhaustion
returns `attempts_exhausted`. A failed revision returns immediately, with its
receipt; there is **no automatic replay**, even if it partially changed files.
Native failure, pre-spawn refusal (`not_started`), protocol error, invalid output
and cancellation remain distinct
outcomes. `parallel.fnl` invokes the saved function twice on distinct grants,
replaces the review stage with an ordinary wrapper, and synthesizes the results.

## Caller authority and ownership

Config owns the exact Due executable, profile native permission flags, ordered
profile lists per role, allowed workspaces per role, and read/write access mode.
Profiles accept `claude`/`codex`, optional model and frozen Due's `low`, `medium`,
`high` efforts. A best-effort early check rejects some native path/model/schema flag forms. Caller-owned Codex
`-c` / `--config` options are allowed so the established unattended permission
profile works; Due validates conflicts with its managed cwd, transport, schema
and selected model/effort. Native configuration is trusted caller input, never
script-generated authority. Permission flags must be chosen by the caller and
must agree with the grant's declared mode. This is an unchecked caller obligation:
the host does not compare native permissions with the read/write label. A read
role with write-capable permissions defeats the reader-overlap rule.
Read/write labels control **host admission**, not OS permissions. A stage can
use any granted role, but cannot manufacture new grants or choose arbitrary
settings. Grants apply across the whole invocation, not lexical stage identity.

Canonical ancestor/descendant/equal workspace relations prohibit overlapping
work when either lease writes. Readers may overlap. Leases reserve queued jobs
as well as active ones; conflicting admissions fail instead of waiting or
deadlocking. Disjoint granted directories may run concurrently. Canonical paths
are checked again at admission and immediately before spawn. This detects path
replacement, but does not eliminate filesystem TOCTOU races, hard-link aliases,
mount aliases or writes a native process performs outside its declared workspace.
Separate CLI invocations do not share locks. The caller must isolate those.

`max_active` (1..32) and `max_jobs` (0..128) are global across nested compositions.
A permit covers spawn through collection. Function compositions have shared
limits of 8 levels, 64 active and 256 total continuations, 32 functions per call.
Scripts have no detached-start or transferable-handle API. Rust retains every
job task independently of Lua's async continuation; errors close admission and
join all tasks, including those cancelled before spawn. Receipts stay in
admission order and include a separate completion sequence.

The library entry point is `workflow::execute(config, Mode, input, artifacts,
Cancellation)`. Each invocation owns one foreground Rust runtime thread. Clone
the cancellation token, call `cancel()`, then await `execute` to collect partial
results. Dropping that future also cancels and synchronously joins the thread,
so it cannot detach children; use the token to avoid blocking an executor thread
during cleanup. This owner thread is local implementation machinery, not a
persistent service. Both CLI SIGINT and SIGTERM use the same cancellation path. The first external
cancellation or fatal failure determines the final status and message, ordered by
monotonic instants. A later signal during cleanup cannot erase an earlier failure;
`cancellation_requested` records the external request on stopped invocations.
Cancellation observed before the final status is computed overrides a just-completed
successful output, including while owned jobs are being joined.

Due receives cooperative **SIGTERM to its PID only**, not a process-group kill.
Its existing provider adapter owns native cleanup. Finocchio waits for Due to
exit and joins collectors; it does not force-kill Due after an arbitrary timeout.
A deliberately uncooperative Due executable can therefore delay cleanup
indefinitely. The caller must select a trusted compatible binary. Killing the
Finocchio process with SIGKILL cannot provide graceful cleanup guarantees.

If Due has exited but another process still holds stdout/stderr open, collectors
stop after a 300 ms post-exit drain window. The native error/result and bounded
partial output survive with explicit incomplete-capture errors. Finocchio does
not know or claim ownership of such orphan PIDs, and cannot promise their cleanup.
The hostile fixture explicitly owns/releases its synthetic orphan. Useful native
failure takes precedence over capture incompleteness; neither is discarded.

## Public Due adapter, artifacts and value bounds

The child command is exactly:

```text
/caller/selected/due run --request - --output json --trace-file HOST_PATH
```

The request uses public schema version 1, caller-selected provider/model/effort,
caller permission flags, canonical workspace cwd, host schema path and
`ephemeral:true`. No private flags, `current_exe`, profile-name lookup, native
binary override, authentication change, or Claude/Codex wire implementation is
used. An installed frozen Due is supported by this public boundary. The exact
path is recorded; this is not executable-content pinning or a version probe.

Fresh directories/files use exclusive creation (0700/0600). Each admitted job has
`admitted.json`, `request.json`, `output.schema.json`, `started.json` if spawned,
`exit.json`, raw `result.json`, `stderr.txt`, trace snapshot `trace.jsonl`, and
`receipt.json`; the final invocation writes `workflow.json`. Admission records
module, role, grant, concrete selected profile/settings and exact Due path.
Receipts retain validation, result presence, native error, exit/PID, capture
errors and trace summary. The full native result is in the individual receipt.
Observed model is **null**: the Due run contract does not report one, and this
spike does not infer it from requested settings or parse model names from traces.

Stdout/stderr memory and retained captures are each bounded to 1 MiB. Excess
triggers sticky shutdown while collectors continue draining during cleanup.
Due exclusively creates its own trace file, so a pre-created FIFO cannot be used
with the installed public contract. A private spool is monitored every 10 ms;
excess triggers cancellation. After exit at most 1 MiB is copied into an exclusive
snapshot and the spool is removed. This bounds **retained capture**, not live
trace disk usage; the spool can overshoot between polls or while Due cleans up.
Late writers can hold the unlinked spool inode. Use OS quotas for a hard bound.
These limits are per job/stream; the total job cap bounds multiplicity, not a
single global 1 MiB artifact quota. Missing/truncated traces are explicit errors.

JSON crosses Rust/Lua only through owned immutable `Data`, never mlua serde's
native-table conversion. `null` stays distinct from missing, empty maps/arrays
stay distinct, and arbitrary-precision numbers remain data. New `data.pack`,
`data.object` and `data.array` construct snapshots from ordinary values plus Data
nodes. Empty native tables require an explicit object/array constructor. Cycles,
foreign userdata, functions, nil, multiple outcomes, non-finite floats, mixed or
sparse keys and metatables are rejected. Conversion has incremental allocation
guards, a 1 MiB serialized-value ceiling and depth 32. Existing standalone JSON
parse semantics remain unchanged; integrated boundaries additionally enforce
these limits. JSON does not magically provide native table destructuring.

## Narrow integrated VM and known tradeoffs

The integrated VM has no OS, IO, package, debug or user coroutine libraries. It
removes loaders, metatable mutation/introspection, xpcall, table.sort and Lua
pattern callbacks/loops; table movement operations are bounded and empty string
replication short-circuits. The same contract library is installed before those
controls. Pure Lua functions and host-owned async continuations can yield normally.
One shared 1,000,000-instruction budget and 32 MiB Lua allocation limit apply to
initializers and runtime work. Every 1,000 instructions yields to the outer owner,
which enforces sticky cancellation/budget exhaustion even through `pcall`.
The pinned `=__mlua_async_poll` implementation wrapper is exempt to avoid charging
idle waits or disrupting nested wakeups, following the existing Due workaround.

This is a narrower integrated environment than `runtime()` used in the earlier
standalone probes. Neither instruction hooks nor these selected controls are a
general security sandbox or hard preemption for every C/Rust call. Compiler C
loops, allocation outside Lua, synchronous schema/JSON work and native permission
configuration retain their documented trust boundaries. Hostile tests run in
deadline-supervised child processes and preserve artifacts on failure.

Duplicated code/design for now: the no-$ref schema subset is adapted from
`src/workflow/contract.rs`; VM restrictions/hook scheduling from
`src/workflow/lua.rs`; exclusive receipts, cancellation/draining and the Darwin
spawn mutex from `src/workflow/process.rs`; shared admission and deterministic
collection from `host.rs` and composition tests. The launcher is intentionally
new because root Due is binary-only and launches itself with private flags.
Every adapter spawn on Darwin holds the one process-wide mutex across pipe
creation through exec, addressing the #19 concurrent pipe inheritance race.
Other processes/libraries outside this adapter do not share that mutex. The
spike owns maintenance of these small duplicated controls until a suitable public
library boundary exists; no second provider implementation is copied.

## Offline evidence and remaining gates

`tests/integrated.rs` covers inspection sentinels, caught initializer effects,
direct and reused reviewer, bounded mutation policy, outcomes and schemas,
actual barrier overlap/reversed completion, nested shared caps, both signals,
completed/interrupted/queued receipts, library cancellation/drop, script errors,
workspace aliases/replacement, stage replacement, rejected privilege overrides,
immutable outcomes, hostile loops/cycles, bounded captures, orphaned pipes,
early request-intake exit, exclusive artifacts and outside-checkout invocation.
Tests use barriers and receipts for ordering claims, not sleep duration guesses.
Deadline durations are emergency test limits, not claimed timing guarantees.

The original 36 acceptance tests remain required. This integrated slice supplies
part of chunk 1's invocation and chunk 2's synthetic harness evidence. It does
**not** finish the pure findings transformation tool, general tooling/diagnostic
repair trial, authoring trial, or product selection. The caller subsequently
verified one real-provider cycle; see [live-evidence.md](live-evidence.md). See [live-trial.md](live-trial.md) for a caller-owned bounded live pair.
