# Experimental Lua / Rhai workflows

The optional Fennel follow-up is documented in
[workflow-fennel-spike.md](workflow-fennel-spike.md). It adds a compiler front end,
mixed Lua/Fennel modules and an explicit v2 returned-function entry contract;
the v1 behaviors below remain available.

`spike/lua-workflows` extends the completed Rhai spike `f6aec6e`. It retains both
interpreters behind independent, optional features. Default builds expose neither
workflow commands nor workflow guidance. Due/wrapper pins and native permissions
are unchanged. This is a bounded foreground experiment, not a production platform.

## Build and select

Work offline and keep feature configurations in separate target directories:

```sh
# On this host select the actual compiler as well as cargo for MSRV checks:
due_toolchain_bin="$(dirname "$(rustup which --toolchain 1.90 rustc)")"
export PATH="$due_toolchain_bin:$PATH"
cargo test --locked --offline --no-default-features --target-dir target/default
cargo test --locked --offline --features workflow-spike --target-dir target/rhai
cargo test --locked --offline --features workflow-lua-spike --target-dir target/lua
cargo test --locked --offline --features workflow-spike,workflow-lua-spike --target-dir target/both
```

Use current rustfmt for repository formatting; verify MSRV separately. No install
is needed. `workflow-spike` includes Rhai 1.26.1, `workflow-lua-spike` includes
mlua **0.12.2**, with default features disabled, `lua54`, `vendored`, `async`,
`serde` enabled. mlua's MSRV is 1.88; Due remains 1.90. A Lua-only dependency graph
has no Rhai. The vendored `lua-src` crate also carries other Lua versions; the
selected runtime is Lua **5.4**, not the version in that crate's package name.
The pinned Rhai `internals` feature exposes its AST visitor to check reserved
bindings without the previous handwritten lexer. Normal syntax, interpolation
and data keys are parsed by Rhai itself.

The entry's `.lua` or `.rhai` extension selects the language; every declared module
must match, except that the Fennel feature permits mixed `.lua`/`.fnl` modules.
Unknown extensions and unavailable runtimes fail before receipts or
children. There is no fallback, dynamic filesystem resolver or native Lua loader.

```sh
./target/lua/debug/due workflow check examples/workflow/workflow-lua.json --input input.json
# Only within existing authorization for the declared provider profiles:
./target/lua/debug/due workflow run examples/workflow/workflow-lua.json --input input.json --artifacts ./new-receipt
```

Check resolves binaries and schemas and compiles **all** declared source files,
but runs no entry or module initializer and starts zero agents. It cannot prove
dynamic imports, branches, admission budgets, or final output. Ordinary OS signal
handling remains in place during check, including blocked source reads.

## Same jobs, two continuations

The [embedded guide](../embedded-skills/workflow/SKILL.md) specifies the manifest,
contracts, limits, artifacts and API. Both adapters pass checked JSON and opaque
handles into one Rust ownership layer and one subprocess supervisor. Every child
is the current executable running ordinary `due run` with the existing profile
request, private schema snapshot and bounded child trace. No script object can
supply executable paths, cwd or arbitrary native arguments. Streaming and capture
remain in Rust. Native envelopes are preserved unchanged beside independent
answer-validation evidence.

Rhai runs on one dedicated blocking orchestration worker, blocking that worker
on host futures. Lua evaluates with `mlua::Function::call_async` and registered
async `run/parallel/wait/wait_any/cancel/require` callbacks. Waiting yields the Lua
continuation to Rust; Lua is not run inside `spawn_blocking` or `Handle::block_on`.
Rust tasks keep supervising children even when a script is busy, throws, exceeds
its budget, drops a handle, or stops being polled.

The shared host has at most 64 invocation-local reservations and 8 active child
slots. Ready slots are granted in admission order, without fixed-size batch
barriers. It is deliberately not a durable scheduler. Every admitted outcome is
retained, including jobs cancelled before spawn, even after receipt/log errors.
The first fatal cause is recorded immediately by collectors or host operations,
not deferred until pipes finish draining. Disk errors can prevent an individual
file being written; the stdout envelope still contains the collected outcomes.

`due.start` returns after admission. `due.wait` is cached and repeatable.
`due.wait_any` selects the earliest **Rust-observed completed** job among its
arguments; its outcome contains `id` and `completion_seq`. The caller removes
that ID to consume the next completion. `due.cancel` cancels only its job and
awaits cleanup. A completed outcome wins the cancellation race and is unchanged.
Normal return with outstanding work **fails**, cancels/joins it, and retains all
receipts. Signals and fatal errors stop admission and stop queued work spawning.

## Lua data and restrictions

Use `json.null` for explicit null, `json.array()` for an empty array, and
`json.object()` (or `{}`) for an empty object. Constructors also accept a plain
table and validate its keys. Dense positive integer tables become arrays;
string-key tables become objects. Mixed/sparse/cyclic tables, invalid UTF-8,
unsupported userdata/functions/threads, and nil final output fail conversion.
Null is mlua's serde sentinel; array identity uses its serde metatable. Conversion
checks raw keys before serializing, preventing serde heuristics from discarding
mixed fields. Missing keys remain nil. Integers stay signed i64 without an f64
round trip; float values must be finite, and integral floats outside +/- (2^53-1)
are rejected. Lua integer arithmetic has Lua 5.4's wrapping behavior; this host
does not redefine arithmetic. Use `math.mininteger/maxinteger` or JSON input for
integer endpoints rather than an out-of-range decimal literal parsed as a float.

Lua has a 16 MiB VM allocator ceiling and a shared 100,000-instruction hook budget
across entry, imported initializers, functions and mlua glue. Every 1,000
instructions the hook yields; the outer Rust future checks the sticky count and
cancellation flag before and after polling, and never resumes an exhausted VM.
`pcall` cannot catch this termination. The memory limit also applies during check.
Memory allocation errors can be caught, but do not reset the instruction budget.
These counts are not equivalent to Rhai operations, and neither VM limit caps
native provider resource use.

The script surface omits io/os/debug/package, eval/load/loadfile/dofile, native C
FFI and loaders, print/warn, collectgarbage, metatable mutation/inspection,
user-created coroutines and xpcall. It also omits table.sort and string
find/match/gmatch/gsub/dump. C callbacks and xpcall error handlers can enter
non-yieldable Lua execution; omitting them prevents catch/resume loops from
trapping termination inside one poll. Pattern matching in C is also outside the
instruction hook. Ordinary Lua functions, closures, tables, loops, pcall, basic
string/table/math/UTF-8 operations remain. Async wrappers capture mlua's internal
coroutine yield privately before the public coroutine table is removed. This is
a restricted standard library, not a custom syntax or scripting dialect.

`table.move`, `table.insert` and `table.remove` retain their normal operations
through bounded host wrappers: at most 4096 moved items or resulting sequence
items. Their C loops do not trigger instruction hooks, and sparse integer keys
can imply enormous sequence lengths without consuming much memory. Checked
integer arithmetic rejects oversized move spans before calling C. These bounds
are separate from the VM instruction budget.
The empty-string/empty-separator case of `string.rep` returns immediately;
Lua 5.4 otherwise repeats an allocation-free C loop even for an empty result.

At run time only, `require("patterns/v1")` and manifest-declared `local/NAME`
modules initialize once in a bounded **effect-disabled** context and must return
a table of functions (at most 128). Nested declared imports and captured constants
work; cyclic imports fail. An attempted host effect during initialization is fatal
even if caught. Rhai instead extracts function definitions and discards all
module top-level statements. This is an intentional semantic difference.

## Examples and evidence boundaries

The review examples exist in [Lua](../examples/workflow/review.lua) and
[Rhai](../examples/workflow/review.rhai), including local substitutions and the
bundled pattern library. [component.lua](../examples/workflow/component.lua)
shows a higher-order phase wrapper without a new host primitive.
[arrival.lua](../examples/workflow/arrival.lua) and
[arrival.rhai](../examples/workflow/arrival.rhai) show completion-driven branching,
replacement work, and cancellation. Set entry accordingly and use a boolean
`true` output schema for their raw outcome object. Real providers do not promise
which review finishes first or that cancellation wins a race.

`tests/workflow_handles.rs` uses the same subprocess fixtures and assertions for
both interpreters. Readiness barriers force a fast completion while a slow native
process remains alive, prove replacement starts before that slow process is
cancelled, and exercise sibling/queued cancellation, concurrency bounds, repeated
waits, handle misuse, signal/error/return cleanup and receipt/log failures.
`tests/workflow_lua.rs` checks composition, effect-free check, module restrictions,
JSON edge cases, VM budgets and contract/capture behavior. The original 20 Rhai
cases remain; the queued-error receipt assertion now requires all admitted jobs,
and the combined-error test orders faults explicitly instead of relying on batch
join order. Shared fixture support is in `tests/support/workflow.rs`.

Unresponsive wrapper cleanup can still wait indefinitely; there is no invented
agent timeout, forced-kill fallback or escaped-descendant accounting. Request
`timeout` remains available. Files have private 0700/0600 modes but same-user
native agents are not isolated by them. No server, retries, persistence/resume,
registry, workspace manager or permission policy is added. The historical
[Rhai assessment](workflow-spike-assessment.md) is separate from this branch's
comparison. Live smoke runs, independent Claude review, adversarial probes and
size/dependency measurements belong to the outer caller. No language superiority,
provider-model ranking or production readiness is claimed here.

The outer caller's [Lua/Rhai comparison](workflow-language-comparison.md) records measured costs, independent probes, real smoke runs, the authoring trial and review repairs.
