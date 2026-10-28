# JavaScript workflow comparison spike

This experiment adds a default-off `workflow-quickjs-spike` adapter using
rquickjs 0.14.0. It is stacked on the shared Lua supervisor and the macOS
pipe-inheritance repair at `0a7ce9b` (PR #27). It is not based on the current
stable main branch. No installation or merge is part of this experiment.

The question is whether ordinary JavaScript data and asynchronous functions
make Due workflows easier to author, compose and repair. The existing
[Lua composition contract](workflow-lua-composition.md) is the comparison point.
Rust continues to own job admission, native processes, permissions, capacity,
receipts, cancellation and joins. This is one foreground workflow invocation.

## Build and run

From this spike checkout, use a separate build directory:

```sh
cargo build --locked --features workflow-quickjs-spike --target-dir target-quickjs
target-quickjs/debug/due workflow --help
cargo test --locked --features workflow-quickjs-spike --test workflow_quickjs
```

The tests use synthetic native providers and need no model credentials. They
exercise the actual Due child process path. No Node installation is required
for the JavaScript runtime; rquickjs builds its embedded C engine.

The [complete example](../examples/workflow/quickjs/README.md) supplies a manifest,
schemas, two concurrent instances of the same saved pipeline, one custom review
stage, and a final synthesis. The executable also embeds the authoring guide:
`due skills show workflow-quickjs`.

## Ordinary functions and explicit ownership

JavaScript entries use manifest version 2 and export a default function taking
a scope-bound `due` capability and input. Modules use ES module exports and
manifest-declared import names. The same saved function can run directly or as
one of several concurrent components:

```js
async function pipeline(due, config, stage) {
  const outcomes = [];
  for (const name of ["design", "implement", "review"]) {
    due.phase(name);
    const outcome = await stage(due, config, name, outcomes);
    outcomes.push(outcome);
    if (!outcome.ok) return {ok: false, stopped_at: name, outcomes};
  }
  return {ok: true, outcomes};
}

async function stage(due, config, name, previous) {
  return await due.run({
    id: `${config.prefix}-${name}`,
    profile: config.profile,
    prompt: `${name}: ${config.task}\nPrevious outcomes: ${JSON.stringify(previous)}`
  });
}

export default async function(due, input) {
  return await due.concurrent("pipelines", [
    child => pipeline(child, input.first, stage),
    child => pipeline(child, input.second, stage)
  ]);
}
```

Profiles, workspace paths and native grants remain caller-owned manifest data.
Parallel writers need explicitly isolated workspaces. `due.concurrent` supplies
each callback with its own capability; pass it through ordinary function calls.
Capturing a different scope's capability does not transfer its ownership.

This differs deliberately from Lua's implicit current scope. JavaScript's
promise queue can resume interleaved continuations; an ambient mutable scope
would attribute a later run or phase to the wrong component. Explicit capability
passing keeps the host identity stable across awaits.

Native failures produce outcomes that scripts can inspect. A review requesting
changes is a domain verdict inside a successfully executed answer. Neither
case is an automatic retry. Script/host contract failures and parent cancellation
must close admission and join owned children before the final workflow receipt.
Cancellation does not undo completed workspace edits.

## What this comparison can establish

JavaScript directly distinguishes empty arrays, empty objects and explicit null.
The host still needs a strict conversion boundary: unsupported values must fail
instead of disappearing or changing through ordinary JSON serialization. Tests
exercise undefined, sparse arrays, extra array keys, cycles, nonfinite and unsafe
integral numbers, BigInts, functions, accessors and unexpected prototypes.

Promises express suspended composition, but they do not supply native-process
ownership or cancellation. Scope cleanup, abandoned work, caught fatal host
errors, microtask loops and errors after an await need their own acceptance.
The engine's interrupt checks and memory limit are separate from native job
limits; a quiet agent wait must not spend the script's compute allowance.

QuickJS provides ECMAScript. Node built-ins, browser APIs, package installation
and automatic network imports are outside this adapter. Imports are resolved
from the manifest's captured sources. Compilation checks syntax without running
module initializers or launching an agent; it cannot prove all dynamic paths.

This spike does not establish that agents write better JavaScript. That requires
a separately controlled authoring and repair comparison with equivalent tasks,
examples and acceptance tests. Implementation and adversarial testing measure
runtime behavior and integration cost, not comparative model performance.

## Deliberate restrictions

- One blocking orchestration task drives the engine and its promise queue. The
  Rust runtime continues supervising native jobs and handling parent signals.
  Each concurrent component receives a distinct `due` capability. A closed scope
  cannot admit more work, and handles cannot cross scopes or enter JSON.
- Await all work. Returning with pending promise jobs or host work fails; there
  is no detached/background success path. A promise with no remaining mechanism
  to settle fails as well. Expected native failures remain inspectable outcomes.
- The runtime caps memory at 16 MiB and stack at 512 KiB. Its 100,000-unit compute
  allowance counts engine interrupt checks and successful job-progress attempts;
  these are not Lua instruction units. Waiting on a quiet native job does not
  spend the allowance. Native run limits and deadlines remain separate.
- `Promise`, `Promise.prototype`, `Function.prototype`, `Object.prototype`, and
  `Array.prototype` are frozen to protect runtime machinery. Ordinary array
  mapping, flattening, sorting, strings, and async functions work; prototype
  patching and libraries requiring it are outside the supported surface.
- JSON values obey the shared 32-level, 4,096-array-item, 256-object-entry, and
  1 MiB limits. Integral numbers outside JavaScript's safe range are rejected on
  input and output. Negative zero serializes as zero. This is JSON fidelity,
  not arbitrary JavaScript object or exact numeric representation fidelity.
- Source filenames survive errors after an await. Diagnostic formatting skips
  script getters and uses the captured native Error stack getter. Some cyclic
  promise-loop failures still report the generic “promise cannot settle” error.

These controls are a bounded workflow contract, not a claim that this embedding
has been audited as a hostile-code sandbox. There is no TypeScript transpiler,
package ecosystem integration, or new filesystem/network permission model here.

## Initial assessment

Native arrays, objects, and null remove Lua's JSON tagging conventions from
workflow code. JavaScript still needs strict validation: ordinary stringify
would silently omit unsupported values, and JavaScript numbers cannot safely
represent every JSON integer. The benefit is a more familiar authoring surface,
not removal of the contract boundary.

Reusable async functions successfully express the concurrent composition case.
They do not eliminate supervision machinery. In this implementation, keeping
scope ownership explicit and deciding what happens to pending promise work were
more consequential than registering the host functions. Review found and fixed
errors around handled rejections, mutable conversion helpers, escaped scopes,
forged handle containers, hostile error formatting, and delayed work after
return; these now have committed regression coverage.

This establishes a viable second runtime adapter. It does not establish a
language winner, a performance advantage, or release readiness. An eventual
agent-authoring comparison should include repair of a failed composed workflow,
not just writing a happy-path script once.

## Local acceptance evidence (2026-10-28)

On macOS arm64 with Rust 1.90, using the locked dependencies:

| Build/check | Result |
| --- | --- |
| Default-feature baseline | 114 tests passed |
| Lua-only baseline | 160 tests passed |
| QuickJS-only complete suite | 143 tests passed, including 25 JS acceptance tests |
| All experimental features | 230 tests passed; 1 existing Fennel test explicitly ignored |
| Formatting and QuickJS-only/all-features Clippy | Passed; warnings denied |
| Independent review | 32 synthetic probes passed; final error-formatting cases rechecked |
| Shipped composition example | Check and run passed; 7 successful synthetic native runs and receipts |

The ignored Fennel test requires native process inspection outside a restrictive
sandbox. The JS tests cover cancellation with active and queued jobs, a pure JS
loop, completion-order dispatch with replacement work, preserved workspace
effects, JSON failures, module diagnostics after an await, compile-only behavior,
and an 18-second quiet native wait. All provider execution was synthetic; no
model/auth/real-provider integration or agent authoring-quality trial was run.

Controlled release builds used the same checkout, compiler, target, and default
release settings, without an extra stripping step:

| Feature selection | Binary bytes | MiB |
| --- | ---: | ---: |
| Default (no workflow runtime) | 3,156,896 | 3.01 |
| Lua workflow | 10,158,208 | 9.69 |
| QuickJS workflow | 10,654,128 | 10.16 |

QuickJS adds 495,920 bytes (about 4.9%) over the Lua build in this experiment.
This measures the complete adapter plus runtime, not just the interpreter. Both
workflow builds include the shared schema/supervision machinery absent from the
default build. Build times and execution throughput were not benchmarked.
Hosted CI is a separate verification step; the local results do not imply that
GitHub Actions has run successfully. Main and the installed CLI were unchanged.

## Reference points

An [optional Squint authoring proof](workflow-squint-spike.md) compiles `.cljs`
into modules for this unchanged runtime, including bidirectional JavaScript
composition. It remains an external build step and does not add a CLI language.

- [rquickjs async integration](https://docs.rs/rquickjs/0.14.0/rquickjs/)
- [AsyncRuntime controls](https://docs.rs/rquickjs/0.14.0/rquickjs/struct.AsyncRuntime.html)
- [Lua composition and ownership](workflow-lua-composition.md)
- [Existing language comparison](workflow-language-comparison.md)
- [Shared process-supervision repair](workflow-sibling-cancellation.md)

The selected rquickjs features are `std`, `futures`, and `loader`. The
`rust-alloc` and custom-allocator features are deliberately absent because
rquickjs documents its built-in memory limit as ineffective with those allocator
configurations. Its experimental multi-threaded runtime feature is unnecessary:
Rust-owned native jobs supply concurrency while one JavaScript runtime coordinates.
