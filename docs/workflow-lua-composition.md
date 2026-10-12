# Lua function composition spike

This checkout builds on `f1e6633314f3388e84904b4ddcacdf866cb3a7f3` on
`spike/lua-composition`. Lua is the working direction for reusable, independently
suspended functions. This adds composition and provider-review evidence, separate from the
historical language/provider trials in [the comparison](workflow-language-comparison.md).
Features remain default-off. Wrapper pins, installed Due, stable main, ordinary
`due run`, and Fennel's pinned compiler/macro subset are unchanged.

## Public API

```lua
local results = due.concurrent("implementations", {
    function() return pipeline(config_a) end,
    function() return pipeline(config_b) end
})
```

`due.concurrent(group, functions)` takes a group name (1–64 ASCII letters,
digits, `_` or `-`) and a **plain dense array of 1–32 functions**, invoked without
arguments. Capture arguments in closures. Sparse arrays, extra keys, nonfunctions,
JSON-marked tables, invalid groups and empty arrays are errors. The entire input
and continuation reservation are checked before any component executes. There is
no user coroutine, spawn, detach, resume or component-cancel API.

Each function must return **exactly one JSON-compatible value**. Return an object
or array for several values; use `json.null` for null. Nil, no return, multiple
returns, functions, opaque handles, cyclic/mixed/sparse tables, nonfinite numbers
and lossy integral floats are rejected. Existing JSON limits apply (depth 32,
4096 array items, 256 object entries, 1 MiB). The combined result also has a
conservative 1 MiB encoded-size bound, charged as components finish, and must
satisfy the same aggregate depth-32 limit. The result array adds one level:
a component value at depth 31 fits; one at depth 32 is valid alone but fails
when wrapped in the result array. Outcome wrappers and `parallel` result arrays
likewise count toward the global depth and byte limits. These limits are not
reset per component. Each result
is copied at function completion; later sibling mutations do not alter that
snapshot. The returned JSON array follows **input order**, regardless of native
completion order. Interleaving and relative component admission order are not
application synchronization primitives.

Existing `due.start`, `wait`, `wait_any`, `cancel`, `run`, `parallel`, `phase`,
`log`, `json`, and manifest-only `require` remain available. Any component may
call `concurrent` again. `parallel` remains an atomic batch of native job
specifications; `concurrent` composes ordinary functions that can each perform
several runs, waits, transformations and branches. A function's implementation
is unchanged when called directly, or from either a single-element or a
multi-element concurrent group.

Job IDs remain invocation-global and explicit. Supply unique prefixes to reusable
functions; duplicate IDs remain fatal. Group names are labels and may repeat.
No legacy job is silently re-keyed. Job specifications still accept only `id`,
`profile` and `prompt`.

## Execution, ownership and errors

Rust owns one invocation supervisor, one Lua VM, and a bounded tree of async
continuations. It polls Lua cooperatively; native Due jobs run under the existing
Rust capacity queue. The entry is scope 0. Each submitted component is a new
scope; ordinary calls inside that component belong to the same scope. Arbitrary
Lua call frames do not create separate host scopes. Use a single-element
`concurrent` if an independently checked ownership boundary is needed.

A scope owns its nested components and all jobs admitted within it. A successful
nested call has joined its child scopes before returning data. Before a component
returns to its caller, its Rust job tasks are joined. Returning with an active or
queued job is a **workflow failure**: all invocation work is cancelled and joined.
Completed jobs may have dropped handles; they are still joined. Throwing from a
component or aborting the invocation also cancels and joins active/queued work.
Returning with unclosed descendant components is also fatal, even with no native
jobs. Pending-group drop guards make abandoned composition futures sticky failures;
outer finalization independently rejects leftover components on a success path.
During global failure, scope closure is deferred until outer cleanup has joined
all native tasks, then proceeds descendant-first. A parent cannot emit
`scope_finished` before its descendants close. Scope completion events occur after cleanup. There is no detached-work success
path, including dropped Lua handles or component futures.

Handle operations are restricted to the **scope that admitted the job**, even
after completion. Ancestor-to-child, child-to-parent and sibling crossing through
captured variables or module state is fatal. A handle is still valid through
ordinary function calls inside its own scope. Return copied outcomes across
scope boundaries, never handles. This restriction prevents two scopes from
claiming ownership and avoids scope-lifetime-dependent handle semantics.

Failed native execution, invalid/missing structured answers and explicit
single-job cancellation produce ordinary outcomes with `ok=false`. Scripts can
examine them and choose whether to continue. In contrast, invalid host inputs,
undeclared profiles, duplicate IDs, handle crossing, resource/admission limits,
receipt/I/O/collector failures, an uncaught component error, malformed component
results and dangling work stop the **whole invocation**. Existing module rules
remain: all initialization effects are fatal, including attempts with arguments
that cannot be converted. Host APIs accept raw Lua arguments before checking
effect availability and routing conversion errors through the admitting scope's
sticky failure path. Host-result JSON validation and Lua conversion failures are
also sticky, including the aggregate returned by `parallel`; catching an error
cannot admit a later native job.

Ordinary `require` errors (including name conversion, missing imports, cycles
and invalid module returns) are recoverable only at the **plain v1 Lua root**.
This deliberate compatibility exception also applies to ordinary nested imports
within that root's initializers; attempted Due effects are still fatal. Component
scopes in either version, v2 roots and bundles containing Fennel always use sticky
loading failures. Ordinary `pcall`
inside a function can handle its own Lua errors before they escape the function.
Allocation failure escaping a component is fatal as well.

A `pcall` around `concurrent` cannot turn these sticky failures into successful
workflow completion. Whole-invocation signal cancellation and instruction
exhaustion terminate at the outer Rust polling boundary, including through
nested `pcall`. No post-cancel host admission is allowed. Rust finishes child
cleanup before emitting the terminal workflow envelope. Cancellation is **not
rollback**: workspace writes that happened before cancellation remain exactly
as written, alongside partial native output and receipts.

## Shared limits and state

| Resource | Invocation-wide rule |
| --- | --- |
| Lua allocations | One 16 MiB allocator ceiling for entry, modules and components |
| Runtime instructions | One 100,000-instruction counter for workflow/library code and initializers |
| Async hook yield | Every 1,000 counted instructions, including protected calls |
| Component width | 1–32 per concurrent call |
| Active components | 64 total, including suspended parents, excluding entry |
| Total components | 256 admitted over the invocation, including completed ones |
| Component depth | 8 below the entry |
| Native runs/capacity | The existing manifest `max_runs` and `max_concurrency`, shared at all depths |
| Events/output/source | Existing bounded event, JSON, source and compiler limits remain |

There is no per-component reset of memory, instruction, native run or capacity
budgets. An instruction-count hook charges each workflow/library instruction and yields
at the shared quantum; counting only each coroutine's full 1000-instruction blocks
would grant short-lived functions an extra allowance. Continuation limits also
bound zero-job fan-out, retained host results and scheduler bookkeeping. Rust
and native process memory are not covered by the Lua allocator ceiling.

The VM, globals, imported function tables, lexical upvalues and captured mutable
objects are **shared**, not copied into isolated Lua environments. A suspended
function can observe another function's mutations when it resumes. Module
initialization occurs once per successful import and uncached loading is
serialized with reentrant same-scope imports and cycle detection. Initialization
flags are per scope; an initializer suspended on an instruction hook must not
disable another component's valid Due effects. Initialization still forbids all
Due effects, including `concurrent`. Cached functions can capture mutable state;
the tests deliberately demonstrate that the state is shared. Prefer immutable
configuration and per-call locals, as in the shipped pipeline. Filesystem
isolation is provided solely by caller-selected native profiles.

## Lifecycle and diagnostics

Workflow events, admitted/started/exit job receipts and step outcomes/receipts gain an additive
`scope` object:

```json
{"id":2,"parent":0,"group":"implementations","component":2}
```

`id` is an invocation-local host integer; `parent` is another scope ID (null for
entry); `component` is the one-based input index (0 for entry). These are distinct
from explicit job IDs and the existing native `group` field. `scope_started` and
`scope_finished` are new lifecycle events. Admission/completion events and job
receipts retain the admitting scope's phase. Child scopes begin with an empty
phase; changing a child phase cannot change its parent or sibling. Native trace
paths and child run IDs still belong to the unchanged explicit job IDs, and the
step receipt links that native trace to its scope. Native runner trace contents
are not rewritten.

Poll-local scope guards restore identity when a nested future returns Pending.
Fennel's host-only source line tracking is now per scope; an error after a wait
uses that component's last location, not the last sibling to run. Plain Lua
retains ordinary raw error locations without enabling Fennel's extra line hook.
Dangling-job failures also capture the admitting scope's location, including the
root. An unmapped generated epilogue does not replace an available last workflow
location in that scope. Mapped locations retain the existing approximate/last-location qualification.

The cached mlua 0.12.2 `AsyncThread::poll`, drop/termination wrapper and global
hook implementation were inspected. Dropping an async call can terminate its
private wrapper; it does not join Rust jobs. Join guards therefore return a
partially awaited job task to the supervisor if their Lua future is dropped.
Outer `finish` owns the final joins and closes remaining scopes in descendant
order. A pending-group guard records abandonment without owning native tasks or
replacing an earlier failure or external signal cancellation. Internal tests drop
real pending composition futures, check that nested Lua futures are released,
and drop a pending scope join to verify its task returns to outer cleanup. The
suggested caught allocation-failure/panic path around mlua's async wrapper is a
static invariant concern; that allocator trigger has **not** been reproduced.
There is no abandoned blocking interpreter worker.

A reproduced progress gap matters here: a hook can suspend an ancestor between
polling a child and yielding mlua's pending sentinel. The next poll can consume
that sentinel without polling the child again, losing progress despite the
child's earlier wake. The first repair used a 5 ms polling heartbeat. Independent caller testing then
found that two quiet native waits exhausted the 100,000-instruction budget after
about 16 seconds: repeated mlua wrapper polling was counted as application work.

The final repair excludes the pinned private `=__mlua_async_poll` implementation
chunk from both instruction charging and hook suspension. Its transition from
polling a nested future to yielding Pending stays atomic. Actual workflow and
module chunks have `@` names, remain charged instruction by instruction, and
cannot claim that private source identity through the supported loader. The
recurring polling timer is removed; native wakeups and user-code hook yields
drive progress. Tests cover quiet native waits beyond the old failure point,
nested protected infinite loops, cancellation and short-function budget sharing.
This exception depends on the pinned mlua wrapper identity and must be revisited
when upgrading mlua.

## Plain Lua library and executable example

The complete example is [composition/main.lua](../examples/workflow/composition/main.lua)
and its adjacent manifest, schemas and input. It imports the public
[pipeline library](../examples/workflow/composition/pipeline.lua), runs the same
design → implement → review function twice in `workspace_a` and `workspace_b`,
transforms/sorts findings in Lua, then asks the declared read-only `synthesizer`
profile to synthesize both outcomes. The pipeline stops its own stages on
`ok=false` and returns the failed outcome as data; the other pipeline continues.
The host contains no pipeline, debate or review engine.

Before running, the caller must supply **existing** workspace directories and
edit the manifest profiles to the intended permissions, native provider/binary,
contracts and canonicalizable cwd paths. The example's relative `workspace-a`
and `workspace-b` are placeholders for those existing directories. No script
creates checkouts or workspaces, accepts an arbitrary cwd, widens permissions,
or changes native options. Using isolated directories is a caller responsibility;
the runtime does not detect two profiles resolving to the same workspace.

With the installed cached toolchain and dependencies, from this checkout:

```sh
due_toolchain_bin="$(dirname "$(rustup which --toolchain 1.90 rustc)")"
export PATH="$due_toolchain_bin:$PATH"
export RUSTC="$due_toolchain_bin/rustc" RUSTDOC="$due_toolchain_bin/rustdoc"
"$due_toolchain_bin/cargo" build --locked --offline \
  --features workflow-lua-spike --target-dir target/composition
# Set profiles to existing authorized workspaces before these commands.
target/composition/debug/due workflow check examples/workflow/composition/workflow.json \
  --input input.json
target/composition/debug/due workflow run examples/workflow/composition/workflow.json \
  --input input.json --artifacts target/composition/example-receipts
```

`check` executes no entry/module initializer and starts no agent. The example
`run` uses real providers only when explicitly run by the caller. The pipeline
acceptance checks use fake-native fixtures; separate caller-owned reviews use
real Claude and Codex runs. The artifacts directory must
be new and its parent must exist.

Stage injection uses the same public API:

```lua
local p = require("local/pipeline")
return function(input)
    local config = {prefix="one", task=input.task,
        profiles={design="reader", implement="writer", review="reader"},
        stages={review=function(c, name, previous)
            -- Replace only review. Other stages use p.stage unchanged.
            return due.run({id=c.prefix.."-review", profile=c.profiles.review,
                prompt="Review these stages for regressions: "..json.encode(previous)})
        end}}
    return p.pipeline(config)
end
```

The injected function receives `(config, stage_name, prior_outcomes)` and returns
an outcome with `ok`; it may itself use `parallel`, `start`/`wait` or nested
`concurrent`. Configuration belongs to the caller, and prefixes stay explicit.
Fennel calls the exact same runtime API, for example:

```fennel
(fn [input]
  (due.concurrent :g [(fn [] (due.run {:id :a :profile :reader :prompt input.task}))
                      (fn [] (due.run {:id :b :profile :reader :prompt input.task}))]))
```

The library's `stable_sort(items, less)` is a copying, stable Lua merge sort for
dense sequences, with a strict-order comparator. Its loops and comparator are
instruction-hook interruptible; equal keys preserve input order. It neither
restores `table.sort` nor depends on Python/Node. The example uses it for findings.
C callback sorting/substitution, pattern matching, user coroutines, xpcall,
loaders, I/O/OS/debug and metatable mutation remain unavailable. Existing bounded
`table.move`/insert/remove and empty-string repetition repairs remain. This is a
restricted embedding, not a complete hostile-code or OS sandbox.

## Validation, findings and unfreeze

`tests/workflow_composition.rs` uses deterministic native readiness/release
barriers to establish simultaneous pipeline stages, input order despite reversed
completion, shared capacity and queued cancellation. It also covers stage
injection, nested/direct reuse, failed outcomes, captured module state and
loading isolation, handle crossing, phase/trace identity, copied results,
invalid inputs/results, zero-job width/active/total/depth limits, shared memory
and instruction bounds, interruptible sorting, active-child early return/error,
parent SIGINT/SIGTERM with queued jobs, no post-cancel launches and retained exact
workspace effects. Fennel uses the same composition API and has a per-component
post-wait source-location test. Existing language/handle suites are retained.

During implementation the protected-loop test reached its guard deadline before
the initial heartbeat repair. Its guard sent SIGTERM and the host joined the children;
it was not a real-provider timeout. A separate initial memory fixture incorrectly
expected failure after releasing its large values; the corrected fixture retains
values across components to prove the shared allocator ceiling. Passing reruns
are new local evidence, not remote CI or paid-model acceptance.

The initial optional Clippy attempt failed with E0514: the Homebrew Clippy used
rustc 1.99 while dependencies were built with the requested Rust 1.90. The
previously selected `1.90.0` alias lacked Clippy. The review repair uses the
existing `1.90-aarch64-apple-darwin` directory, which includes matching
`cargo-clippy`; nothing was installed or globally reconfigured.

Review repair regressions additionally cover table/invalid-UTF-8 host arguments,
initializer effect checks before conversion, the explicit v1-root `require`
exception, three valid 400,000-byte answers exceeding the `parallel` aggregate
bound, all four single-outcome APIs rejecting over-depth wrappers, no post-catch
native admission, depth-31/32 component boundaries, and Fennel dangling-work
locations unaffected by siblings. The synthetic fixture emits bounded answers;
no real models are needed. Deterministic internal tests cover abandoned futures,
unclosed zero-job descendants, signal/first-cause preservation, returned native
join ownership and descendant-first lifecycle events. These tests do not claim
to reproduce a Lua allocator abandonment trigger.

Final caller verification used Cargo/rustc/rustdoc **1.90.0**, matching Clippy,
explicit `RUSTC`/`RUSTDOC`, `--locked --offline`, and checkout-local
`--target-dir target/caller` (four test threads):

| Selection | Complete suite result |
| --- | --- |
| Default | 114 passed |
| Rhai | 146 passed |
| Lua | 157 passed |
| Fennel | 180 passed, 1 process-inspection test ignored |
| All features | 201 passed, 1 process-inspection test ignored |

The ignored compiler signal/cleanup test was then explicitly enabled with
host process-inspection permission and passed. Matching-toolchain all-target
Clippy passed with warnings denied for all five configurations. The Node caller's
22 tests, formatting and whitespace checks passed. The composition suite includes
the 20-second quiet-wait regression and protected-loop progress tests. No polling
timer or implicit native-work deadline was added.

Independent caller probes exercised the built CLI rather than its Rust internals:
21 lifecycle/limit/error cases, three unchanged-pipeline/custom-stage/cancellation
cases, and three review regressions all passed. They checked readiness barriers,
exact writes in separate caller-created workspaces, preserved cancellation effects,
absence of later admissions, receipts and process cleanup. Another check validated
269 native trace records and scope links across 45 step/admitted/started/exit
receipt sets. The quiet-wait reproduction failed after 15.706 seconds before the
repair and completed both 20-second jobs in 20.079 seconds afterward. Four depth
boundary probes confirmed the documented direct-versus-aggregate limit. Final
host inspection found no processes matching this task's checkout/artifact paths;
this is bounded evidence, not a guarantee about escaped descendants.

The installed Due delegated implementation and repairs to Codex. Through the
candidate workflow API, the caller also ran a controlled, read-only Claude/Codex
review comparison using the same prompt, schema and frozen source-file hashes.
Codex's first attempt failed native preflight because the context directory lacked
Git metadata. Due retained that failure as data while Claude continued. After
checking source hashes and initializing Git metadata, the caller explicitly ran a
fresh Codex-only workflow with unchanged permissions. Both successful arms
overlapped in time across **two** workflow invocations; the original group did
not contain two successful model runs.

The caller selected Codex's response as better supported on this bounded review:
all three findings were independently reproduced (catchable host argument errors,
oversized aggregate results allowing later admission, and missing Fennel locations
for dangling work). Claude's useful abandonment invariant was also retained;
its proposed allocator/panic trigger was not reproduced. Both sets of useful
findings led to repairs and regression tests. This is not a language comparison
or a general provider ranking. Requested model/effort were omitted and observed
settings remain unknown.

A final independent Claude review through installed Due approved the repaired
source, with no remaining correctness finding in the reviewed paths. Its
machine-specific documentation-path note was fixed; optional test-helper layout
cleanup was deferred. All five successful implementation/repair/review runs had
valid result/answer contracts, complete trace sequences and terminal events;
raw evidence remains private. The caller verified the candidate's embedded
workflow guide against its source. Main, the installed CLI and wrapper pins
remain unchanged; this work is a draft branch, not a release.

Native cleanup still can wait indefinitely on an unresponsive child and does
not guarantee containment of escaped descendants, process crashes or SIGKILL.
[#19](https://github.com/joshrotenberg/due/issues/19) remains unresolved: the
nested hook progress finding does not diagnose that older native readiness timeout.
Before unfreezing, native cleanup investigation including #19, hosted/platform
CI, and broader authoring/real-provider acceptance remain. Local passes and a
review approval do not establish those gates. There is no daemon, persistent
scheduler, resume, automatic workspace creation, second Lua runtime, new Fennel
compiler capability, or promise to ship multiple interpreters.

First-use simplification is a future UX decision: a library might supply default
stages/contracts, make gating opt-in, or shorten profile/manifest authoring.
This spike keeps the existing required input/output/per-job contracts and
truthful native outcomes. Those possible UX changes are not mandatory scope for
this implementation and do not justify weakening permissions or ownership.
