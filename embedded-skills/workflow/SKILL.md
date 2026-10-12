---
name: workflow
description: Author and check an experimental bounded Lua or Rhai workflow with caller-declared Due profiles, schema gates, and private subprocess receipts.
---

Use this only with `workflow-spike` (Rhai) or `workflow-lua-spike` (Lua),
or both. The entry extension `.rhai` / `.lua` selects the runtime; unavailable
languages fail before children. Default builds expose neither. This is a
Unix execution spike, not a production release or an OS sandbox. For one task,
`due run` is cheaper and simpler. Existing native permissions still apply.

1. Read `due workflow --help`, `due workflow run --help`, and the `run` and
   `contracts` embedded skills. The caller authorizes profiles, permissions,
   workspaces and any model work. Do not add native permissions or delegation.
2. Write a manifest, schemas, input and script as below. Every profile needs an
   answer schema. Prepare workspaces yourself. Use read-only native profiles for
   parallel reviews; a cwd alone provides no isolation. Arbitrary native_args
   remain trusted caller configuration, including their native path semantics.
3. Run `due workflow check workflow.json --input task.json --output json`.
   Check compiles the entry and every declared module and validates definitions
   and input. It evaluates no script or module initializers and starts no agent.
   Builds with `workflow-fennel-spike` also expose `workflow-fennel` guidance:
   bounded compilation executes compiler/built-in macro code in a joined helper.
   It does not prove imports, dynamic branches, job IDs/budgets or final output.
4. Under the caller's existing authorization, use
   `due workflow run workflow.json --input task.json --artifacts new-receipt --output json`.
   The artifact directory must be new, and its parent must exist. SIGINT, SIGTERM
   or SIGHUP stop admissions and signal/wait for active ordinary Due children.
   Check retains normal OS signal handling during source reads; Fennel compiler
   helpers have signal-aware cleanup. See the feature-specific guide.
5. Inspect workflow.json and each step's step.json, result.json, stderr.txt,
   trace.jsonl and exit.json. Verify actual effects separately. Failed or
   cancelled jobs may already have edited files; there is no rollback or retry.

Minimal manifest (JSON v1; unknown fields fail):

```json
{
  "schema_version": 1,
  "entry": "main.rhai",
  "input_schema": "input.schema.json",
  "output_schema": "answer.schema.json",
  "max_runs": 3,
  "max_concurrency": 2,
  "profiles": {
    "reader": {
      "request": {
        "schema_version": 1,
        "provider": "codex",
        "cwd": ".",
        "schema": "answer.schema.json",
        "native_args": ["--sandbox", "read-only"]
      }
    }
  },
  "modules": {}
}
```

Request accepts existing Due v1 request fields except prompt (jobs supply it).
`cwd` defaults to the manifest directory; schema is required. A profile may add
`"binary":"./fake-agent"` beside request for a fake native provider executable.
Otherwise the provider is resolved from PATH before evaluation. Entry, schemas,
input, modules, cwd and binary resolve relative to the manifest directory and
are canonicalized. Sources, input and resolved profiles are snapshotted before
execution; child schema transport uses private snapshots. No process-wide chdir.
Executables and workspaces are referenced, not copied or made immutable.

For this example, input.schema.json is
`{"type":"object","required":["task"],"properties":{"task":{"type":"string"}},"additionalProperties":false}`;
answer.schema.json is
`{"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}},"additionalProperties":false}`;
task.json is `{"task":"Review a synthetic proposal; do not edit or delegate."}`.

```rhai
import "patterns/v1" as p;
let reviews = p::review("review", "reader", input.task);
due::phase("synthesis");
p::answer(due::run(p::job("synthesis", "reader",
    "Reconcile findings: " + json::encode(reviews))))
```

Exact API:

- `due::run(#{id, profile, prompt})` returns one step outcome.
- `due::parallel(group, [jobs])` reserves the entire batch before any launch,
  runs at most max_concurrency children with available slots reused immediately,
  and returns outcomes in input order.
  IDs are globally unique, 1..64 ASCII alphanumerics, `_` or `-`; group uses the
  same grammar. Jobs accept only id/profile/prompt. Prompt is 1..65536 bytes.
- `due::phase(text)` sets subsequent step metadata (at most 256 bytes).
  `due::log(text)` writes private workflow JSONL (at most 4096 bytes).
  Rhai print/debug are discarded; stdout is one workflow JSON envelope.
- `json::encode(value)` performs checked JSON conversion.
- Bundled `patterns/v1` exports `job(id,profile,prompt)`, `answer(step)`,
  `answers(steps)`, and `review(prefix,profile,task)` (two reviews, gated answers).
  `answer` throws unless execution and independent answer validation succeeded.

A step exposes id/profile/group/phase, ok, actual Due exit code/signal, error,
validation {valid,error}, answer_present, and artifact/child run references.
Only an ok step has answer. Native failures and invalid answers are values;
script/host errors stop the workflow, retaining partial receipts. A script may
handle an expected failed outcome explicitly. `p::answer` is the normal gate
before dependent jobs. `ok` says nothing about a task's approved/rejected verdict.
The child v1 envelope is preserved unchanged in result.json and step.json's
child_result. Host validation is beside it, not a rewrite of its
answer_schema_validation field.

The final expression is validated against output_schema. Stdout and workflow.json
use workflow_version:1, workflow_id, status, steps, output_present, and output on
success, or error on failure. Statuses checked and succeeded exit 0; failed exits 1 and cancelled exits 130
(CLI syntax errors exit 2). Failure before directory creation has
no artifacts. Missing final workflow.json indicates an incomplete/crashed run.

Local composition: declare `"modules":{"local/review":"review.rhai"}` and
import `"local/review" as review`. Only function definitions are extracted;
all top-level module statements, imports, constants and effects are discarded.
Import dependencies inside functions. Local functions can call the same due/json
primitives as bundled functions. Module names must be local/NAME; patterns/v1,
due and json are reserved. Host namespace names cannot be rebound. Rhai input is a
constant. No eval, filesystem resolver, shell, filesystem or network API is
exposed. Blocking sleep, timestamps and Rhai's permissive parse_json are absent.
Rhai parses ordinary syntax, including interpolation. Reserved bindings are
checked using its AST; there is no extra lexer. Lua uses ordinary table/global
bindings; changing or dropping a script binding never transfers job ownership.

Schemas use a restricted Draft 2020-12 dialect. Supported keywords: $schema
(exact https://json-schema.org/draft/2020-12/schema), type, enum, const, required,
properties, additionalProperties, items, allOf, anyOf, oneOf, not, minimum,
maximum, exclusiveMinimum, exclusiveMaximum, minLength, maxLength, minItems,
maxItems, minProperties, maxProperties, uniqueItems, title, description. Boolean
schemas work. Every other keyword, including $ref, $id, format and pattern, is
rejected; network/file schema resolution is disabled. JSON null maps to Rhai (),
missing map properties error. Integers must fit i64, floats must be finite, and
integral floats outside the exact +/- (2^53-1) range are rejected. Character,
blob, function-pointer and other non-JSON values are rejected.

Fixed limits: 1..64 runs, 1..8 concurrency (no greater than max_runs), 1..32
profiles, <=32 local modules, 1 MiB per source/JSON value/child stdout/stderr/trace,
32 JSON/schema nesting, 100000 Rhai VM operations, 32 Rhai call depth, 4096 array items,
256 object entries (and Rhai variables), 128 Rhai functions and 64 Rhai module imports. Workflow logging
has a 2048-record/1 MiB budget. Work uses available slots in admission order. Output
overflow stops admissions, signals children, drains pipes, and retains truncated
bytes plus the error; trace overflow is recorded by the child's trace.incomplete.
No provider deadline is invented; caller request.timeout remains available.
Cancellation awaits wrapper cleanup; there is no forced-kill deadline for an
unresponsive Due child or escaped descendant accounting. Native processes can
consume resources beyond these host limits. Artifacts have private Unix modes,
not protection against a provider with the same user permissions. Do not put
untrusted writable workloads around the receipt directory. This spike does not
verify workspace overlap, effects, authentication, native flag support or costs.
No retries, resume, persistent scheduler, server, workspace creation or registry.


## Shared run handles and cleanup

Lua uses `due.start/wait/wait_any/cancel/run/parallel/phase/log` and `json.encode`;
Rhai uses `::`. They have the same host semantics:

- `start(job)` admits one job and returns an opaque, cheap Rust-backed RunHandle.
  It does not wait for completion. A full concurrency limit queues the job under
  the invocation's max_runs budget. IDs remain globally unique across all calls.
- `wait(handle)` returns the cached outcome after cleanup and recording. Repeated
  waits return the same value without additional provider work.
- `wait_any([handles])` (Lua `{handles}`) returns one outcome with `id` and a
  zero-based `completion_seq`. Rust assigns the sequence when a subprocess has
  exited and its output collectors have joined, before receipt writes. This is
  observed completion order, not a claim about native exit timestamps. Already
  completed handles are eligible; the smallest sequence wins, even when passed
  after a later completion. Remove the returned ID yourself to consume arrivals.
- `cancel(handle)` requests cancellation and awaits that job's cleanup. It does
  not stop siblings or global admission. A queued job never spawns. A previously
  completed job returns its unchanged cached outcome. A completion racing a
  cancellation may still succeed; `cancel_requested` records the request as seen
  at collection and does not assert that a signal won the race.
- Handles belong to exactly one invocation, cannot be serialized as JSON or
  created from IDs/maps, and can be copied/reused within their owning scope.
  Lua concurrent components cannot use ancestor, sibling or descendant handles;
  ordinary calls inside the same component can share them. Rhai uses one scope. Empty
  wait_any lists, duplicates, foreign handles and malformed handles error.
  Scripts cannot obtain foreign handles through the provided APIs. No events
  are consumed by wait_any and no implicit cursor or timeout exists.

run is start+wait; parallel validates and reserves its entire batch atomically
before launch, then awaits all outcomes in input order. Final `steps` also stay
in admission order; completion_seq and step_completed JSONL expose arrivals.
Check performs none of these effects. Admission, collection and persistence
failures are fatal and sticky, including when a script catches the callback
error. Native failures and invalid answers are outcomes; callers may branch on
`ok`. Lua host argument/result conversion failures are sticky, including through
pcall. Ordinary require errors remain recoverable only at a plain v1 Lua root;
component/v2/Fennel loading errors are sticky. Initialization effects always fail.
Rhai interpreter argument-type errors remain ordinary catchable language errors.

Every admitted job remains Rust-owned. A normal return with outstanding jobs
fails the workflow, cancels/joins them, and keeps their outcomes. Script errors,
budget exhaustion, signals, dropped handles and secondary log/receipt failures
also stop admission and cancel/join unfinished work. Closing the VM is never the
cleanup mechanism. All outcomes survive in memory/stdout even if an individual
receipt cannot be persisted. The first fatal cause remains the error; failed
persistence of the final envelope is separately `artifact_error`.

## Lua modules, JSON and budgets

Use the same manifest, with entry `main.lua` and declared `.lua` module paths:

```lua
local p = require("patterns/v1")
local handles = {
    due.start(p.job("a", "reader", input.task)),
    due.start(p.job("b", "reader", input.task))
}
local first = due.wait_any(handles)
local other = first.id == "a" and handles[2] or handles[1]
local replacement = due.start(p.job("replacement", "reader", json.encode(first)))
local cancelled = due.cancel(other)
return {first=first, cancelled=cancelled, replacement=due.wait(replacement)}
```

This raw outcome example needs max_runs >= 3 and a compatible final schema
(e.g. boolean true). The bundled Lua module exports the same job/answer/answers/
review functions as Rhai. A local Lua composition can return
`{review=function(prefix,profile,task) ... end}`. At run time, require initializes
bundled/declared modules once, in a bounded context where all due effects are
disabled, and expects a plain string-keyed function table (<=128 functions).
Captured constants and nested declared imports work; cycles fail. Initializers
never execute during check. Rhai's function-only extraction still discards them.

Explicit JSON null is `json.null` (mlua serde's sentinel), distinct from missing
keys (`nil`). Empty arrays are `json.array()`; empty objects are `json.object()`
or `{}`. Constructors accept plain tables too. Dense 1-based tables become arrays,
string-keyed tables become objects. Mixed/sparse/cyclic tables, other userdata,
functions, threads, invalid UTF-8 and nil final values fail. Checked conversion
preserves signed i64 endpoints and rejects nonfinite/lossy integral floats; it
never coerces integer values through f64. Lua integer arithmetic itself retains
Lua's wrapping semantics. Use JSON input or math.mininteger/math.maxinteger for
endpoints; oversized float literals cannot encode arbitrary exact integers.

Lua is pinned mlua 0.12.2 with vendored Lua 5.4, async and serde. Its 16 MiB VM
ceiling applies to compilation and evaluation. The shared 100000 instruction
budget yields every 1000 instructions; an outer future refuses to resume after
budget exhaustion or cancellation. pcall cannot catch this termination. The
count includes workflow/library code and module initialization, but excludes the
pinned private mlua polling wrapper. Quiet native waiting spends no script budget;
short-lived components cannot reset it. This is not equivalent to Rhai operations.
The private wrapper is also exempt from hook suspension to preserve nested wakes.
An mlua upgrade must revalidate that boundary and the quiet-wait regression.
There is no periodic interpreter polling timer or implicit provider deadline.
Catchable memory allocation errors cannot reset the instruction count.
Lua actually suspends through async callbacks while Rust supervises children.

No io/os/debug/package or native loader, load/loadfile/dofile, print/warn,
collectgarbage, metatable setters/getters, user coroutine controls, xpcall,
table.sort, or string.find/match/gmatch/gsub/dump are exposed. Non-yieldable C
callback paths and C pattern loops are deliberately excluded so they cannot
trap budget termination inside a poll. Internal coroutine yield remains private
to mlua async wrappers. Basic strings/tables/math/UTF-8, functions, closures,
loops and pcall use ordinary Lua syntax. VM restrictions are not a native-process
sandbox. See docs/workflow-spike.md and examples/workflow for complete source
examples and the comparison's explicit limitations.

table.move/insert/remove are bounded wrappers around Lua's C implementations.
Moves may span at most 4096 items; insert/remove require a sequence within the
4096-item limit. Sparse keys do not bypass these checks, and oversized integer
spans are rejected before entering C. These errors can be caught normally;
repeated catches still consume the shared instruction budget.
string.rep returns immediately when both the repeated string and separator are
empty, avoiding Lua 5.4's otherwise allocation-free C loop.

## Lua function composition

`due.concurrent(group, functions)` invokes a plain dense array of 1–32 ordinary
zero-argument functions concurrently and returns copied JSON values in input
order. Capture inputs in closures. Functions may call existing Due APIs and nest
concurrent groups. Each must return exactly one JSON-compatible value; use
json.null for null. The combined array also obeys the 1 MiB/depth-32 limit, so a
component value at depth 32 cannot fit beneath that extra array level.

For the shipped design → implement → review library, declare local/pipeline as
examples/workflow/composition/pipeline.lua, supply caller-approved profiles for
two isolated workspaces, allow at least six runs, and use a compatible output
schema. The complete executable example and synthesis stage are adjacent to that
library; docs/workflow-lua-composition.md describes the experimental contract.

```lua
local p = require("local/pipeline")
local function work(prefix, profile)
    return p.pipeline({prefix=prefix, task=input.task,
        profiles={design=profile, implement=profile, review=profile}})
end
return due.concurrent("implementations", {
    function() return work("a", "workspace_a") end,
    function() return work("b", "workspace_b") end
})
```

The pipeline stays an ordinary function; replace stages through config.stages.
It uses the same API as custom modules. Its Lua stable_sort helper provides an
interruptible copying sort with stable ties; native table.sort remains disabled.

Rust owns the component tree and every job. Returning with active/queued jobs,
abandoning a pending group, or finishing with unclosed descendants fails the
invocation and joins remaining work. Expected failed agent outcomes stay data.
No component detaches, creates a workspace or widens profile permissions. Job
prefixes remain explicit and invocation-global. Cancellation retains earlier edits.

All components share one VM, native capacity, run budget and instruction/memory
limits. The host also bounds total components to 256, active components to 64 and
depth to 8. Globals, module tables and mutable captures are shared: prefer per-call
locals and immutable configuration. Module initialization is serialized and
cannot start agent work. Phase and diagnostic state are per scope. Workflow
events and step/admitted/started/exit receipts carry scope identity and parent
links; scope_finished follows cleanup. Native traces and envelopes stay unchanged.
Fennel uses this same API when its optional frontend is enabled; Rhai does not
expose concurrent function composition in this spike.
