# Fennel authoring spike

Branch `spike/fennel-workflows` builds on Lua comparison
`2202b5f308c2fa97190f8959ce5e65d6e3502f77`. Fennel is an optional compiler front end
for that Lua candidate. Rust still owns the same start/wait/wait_any/cancel jobs,
schema gates, subprocesses, outcomes and receipts. The Lua workflow executes
through `mlua::Function::call_async`; compilation does not create a second job
supervisor or a permanently blocking workflow thread.

`workflow-fennel-spike` depends on `workflow-lua-spike`. No default feature,
wrapper pin, ordinary `due run`, plugin or install-policy change is made. Rhai
remains a separate candidate. Combined features support comparative tests only;
this experiment is not a recommendation to ship Rhai and Lua together.

## Build and use

The runtime uses cached mlua 0.12.2 / vendored Lua 5.4.9 and the embedded Fennel
library. It neither needs nor searches for a Lua/Fennel executable, and fetches
nothing at runtime. To test the actual Rust 1.90 MSRV on a host whose default
rustc is newer:

```sh
due_toolchain_bin="$(dirname "$(rustup which --toolchain 1.90 rustc)")"
export RUSTC="$due_toolchain_bin/rustc" RUSTDOC="$due_toolchain_bin/rustdoc"
"$due_toolchain_bin/cargo" test --locked --offline --features workflow-fennel-spike --target-dir target/fennel
"$due_toolchain_bin/cargo" test --locked --offline --features workflow-spike,workflow-fennel-spike --target-dir target/all
"$due_toolchain_bin/cargo" clippy --locked --offline --all-targets --features workflow-fennel-spike --target-dir target/fennel -- -D warnings
cargo fmt --all -- --check  # current rustfmt
```

A complete example uses these files in one directory:

`workflow.json`:

```json
{
  "schema_version": 2,
  "entry": "main.fnl",
  "input_schema": "input.schema.json",
  "output_schema": "answer.schema.json",
  "max_runs": 3,
  "max_concurrency": 2,
  "profiles": {
    "reader": {
      "request": {
        "schema_version": 1, "provider": "codex", "cwd": ".",
        "schema": "answer.schema.json", "native_args": ["--sandbox", "read-only"]
      }
    }
  },
  "modules": {}
}
```

`input.schema.json` is
`{"type":"object","required":["task"],"properties":{"task":{"type":"string"}},"additionalProperties":false}`.
`answer.schema.json` is
`{"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}},"additionalProperties":false}`.
`input.json` is `{"task":"Review a synthetic proposal; do not edit or delegate."}`.

`main.fnl`:

```fennel
(local p (require :patterns/v1))
(fn [input]
  (local a (due.start (p.job :first :reader input.task)))
  (local b (due.start (p.job :second :reader input.task)))
  (local first (due.wait_any [a b]))
  (local other (if (= first.id :first) b a))
  (local next (due.start (p.job :replacement :reader
                              (.. "Inspect this outcome: " (json.encode first)))))
  (due.cancel other)
  (p.answer (due.wait next)))
```

```sh
./target/fennel/debug/due skills show workflow-fennel
./target/fennel/debug/due workflow check workflow.json --input input.json
# Only with the caller's existing authorization for the declared model work:
./target/fennel/debug/due workflow run workflow.json --input input.json --artifacts new-receipt
```

Profiles/binaries and all three schemas remain required even for zero-job
programs, preserving comparison parity. Use the existing fake-native fixture
for implementation tests, never real providers. The complete translated
[arrival example](../examples/workflow/arrival.fnl) and its
[Lua v2 counterpart](../examples/workflow/arrival-v2.lua) share the same policy.
The [feature sample](../examples/workflow/features.fnl) uses no jobs.
A reusable [Fennel component](../examples/workflow/component.fnl) is called by
[Lua](../examples/workflow/fennel-from-lua.lua) through ordinary exported functions.
For that example set the entry accordingly, declare `local/component`, and use
`answer.schema.json` as the final schema instead of the arrival outcome schema.

## Entry protocol and mixed modules

Manifest **v1** Lua chunks retain direct execution with global `input`; Rhai v1
retains its existing behavior. Manifest **v2** is only for Lua/Fennel: load the
entry with Due effects disabled, require a function result, then invoke that
function once with input using `call_async`. Rhai v2 is rejected. `.fnl` entries
require v2. No existing v1 file is silently reinterpreted.

A v1 Lua bundle may explicitly declare a Fennel function module. Each `.fnl`
file is compiled once before any agent admission. Plain Lua bundles skip the
compiler entirely, including when the feature is present. `.lua` and `.fnl`
modules share manifest-only require, cache, cycle detection, effect-disabled
initialization and plain string-keyed function exports (at most 128). Module
load failures, undeclared imports, cycles, and effects attempted during loading
remain sticky even through pcall. No child can be admitted before v2 invocation.
Input/final/per-step validation requirements are unchanged.

Check parses/compiles every declared source and validates manifests/schemas/input.
It starts **zero agents**, executes **no workflow entry or module initializer**,
and cannot validate dynamic imports or the returned entry value. It **does
execute compiler code and standard built-in macros**, in a disposable compiler
helper process. The helper has no Due bindings or source-visible native effect
API. Host-owned private temporary transport files and the helper process are
part of compilation, so check is not described as executing no code or creating
no OS resources. Source reads retain normal OS signal handling; during compiler
execution check installs signal handling that kills and joins its helper.

## Supported subset and capabilities

The tested subset includes ordinary functions/lambda, closures, partial,
let/var/set, arithmetic, if/when, each/for/while, match/case, comprehensions and
accumulators, sequence/map rest and destructuring, threading macros, doto,
varargs and multiple return values. Standard built-ins run as trusted compiler
code. Workflow components are ordinary functions, with no macro-only Due API.

User `macro`, `macros`, `import-macros`, `require-macros`, `eval-compiler`,
`include`, `lua`, `quote`/`unquote`, `with-open`, `assert-repl`, `macrodebug` and
`global` forms are unsupported and rejected with source locations. The driver
overrides both Fennel dispatch tables **before macro expansion**. This matters
because import-macros can call compiler specials directly before a call hook.
The upstream parser measures AST depth/table nodes; there is no handwritten
lexer or blacklist of words inside strings/comments/data keys. Compiler plugins
and arbitrary options cannot be supplied by workflows. Adding a new compiler
execution path requires another capability/resource audit.

Compilation and execution have separate Lua states. Compiler state includes
metatables, load, protected calls, string patterns and table helpers needed by
the pinned implementation. Its package table is a synthetic preload-only
registry for modules physically bundled in the library. There is **no real
package library, io, os, debug, environment API, native loader or Due API**.
The compiler process receives a cleared environment. Macro searchers are emptied,
paths cleared, and compileString gets an empty compilerEnv and fixed options:
correlate/useMetadata/requireAsInclude/assertAsRepl/toBeClosed all false.
Containment does not rely on allowedGlobals, Fennel's default safe-compiler-env,
or replacing its exported search-module. The default relative-file reader has
no io capability to call. Explicit includes and macro imports are also excluded.

The workflow VM retains Lua's restrictions: no I/O, OS, debug, loader, user
coroutines, metatable mutation, xpcall, table.sort or C string pattern APIs.
Dynamic access through `_G` does not restore removed capabilities; `_G` is
available because built-in matching emits `_G.type`. Library APIs omitted in
Lua are also omitted in Fennel, even if a dynamic reference compiles. This is
not a static type checker for arbitrary table-field calls.

Fennel sequence-rest emits `getmetatable`. For bundles containing Fennel, the
runtime supplies a compatibility function that **always returns nil**, for all
values. It never reveals JSON marker, string or handle metatables, and offers
no custom `__fennelrest`. No generated string rewriting is used. Plain Lua-only
VMs still have no getmetatable. This deliberately limits extensible Fennel rest
semantics to ordinary table rest. Unsupported metatable operations stay absent.

JSON uses the unchanged Rust validator/converter: `json.null` versus missing
nil, explicit `(json.array)` and `(json.object)`, exact signed i64 and finite
floats. Empty Fennel `[]` is an untagged Lua table, hence an empty JSON object.
Mixed/sparse/cyclic tables, opaque handles, nonfinite floats and potentially lossy
integral floats fail. Use input or math.mininteger/maxinteger for i64 endpoints.

## Resource and cancellation boundaries

Limits apply before agents are admitted:

| Resource | Bound |
| --- | --- |
| Fennel source | 128 KiB per file; 512 KiB total Lua/Fennel bundle source, including bundled patterns |
| Files | entry plus at most 32 declared modules |
| Compiler Lua allocations | 32 MiB, including library and built-ins |
| Compiler instructions | 20 million shared across library loading and all compilation |
| Compiler expansion events | 4096 call/symbol/destructure events per file |
| Parsed AST | depth 64; 32768 table nodes per file |
| Generated Lua | 512 KiB per file; 1 MiB total |
| Source mapping / reply | 4 MiB conservative map accounting; parent reads at most 8 MiB reply |
| Compiler wall time | 5 seconds for the whole helper invocation, polled every 5 ms |
| Workflow runtime | existing 16 MiB and 100000 instructions, with 1000-instruction async yields |

The hook limit is sticky and checked after protected compilation returns. It is
**not** the ultimate interrupt boundary: pcall/xpcall can catch hook errors,
and C pattern/table/string loops may never execute another VM instruction.
The parent async task continues observing shutdown and its fixed compiler-only
wall clock. It kills and waits for the helper on cancellation/deadline/error,
including when the parent future is dropped. There is no offloaded compiler
thread to abandon. Source, generated-output and map checks also bound Rust-side
transport. Parser stack failures and memory failures are compiler errors (or a
failed helper exit), never reasons to admit work. Failed compilation creates no
workflow receipt directory. Successful run snapshots are private.

This is a bounded experimental embedding, **not a complete hostile-code or OS
sandbox**: the process runs as the caller, the Rust process/runtime footprint is
not covered by the Lua allocator, and there is no kernel isolation or protection
against implementation vulnerabilities. Capability removal and the tested subset
are essential. Provider deadlines are unchanged. Existing native cleanup may
still wait indefinitely for an unresponsive Due child and does not account for
escaped descendants; the compiler deadline does not change that contract.

## Diagnostics and provenance

Compilation and generated Lua use actual manifest source names, including nested
paths. Successful snapshots retain original Fennel, generated Lua, compiler
version/hash/upstream commit/URL, fixed options, limits, the integration driver,
and generated-line mappings. The compiler library is unmodified; provenance and
MIT license are in [the vendor directory](../assets/vendor/fennel/README.md).
No runtime metadata or compiler require is injected into generated workflow code.

Public `correlate` is approximate and disables the detailed map; it is off here.
The integration deliberately couples to **Fennel 1.6.1 compileString's second
return**: a table indexed by generated Lua line, with `[filename, original line]`
entries. This implementation-dependent return is copied into Rust before the
compiler state is discarded. Synthetic rows can contain nil; those are retained
as unmapped, never fabricated into exact source positions. Upstream macro
locations may also appear. Upgrade requires provenance and map regression tests.

Runtime diagnostics retain raw Lua errors and append mapped locations when
available. A host-only line hook retains the last workflow file/line, including
a tail-called callback after real due.wait suspension and instruction-hook
yields, where mlua's callback traceback loses its script frame. The error text
labels that context as **last workflow location**, not necessarily the origin
of an independent native collector failure. Debug is never exposed to scripts.
Some generated lines have no map; their raw generated location remains useful
alongside the snapshotted output. Arbitrary user error text is not trusted as
proof of a source location; tests assert known file/line pairs after suspension.

## Validation and comparison limits

Tests use only synthetic native children and the existing readiness barriers.
They cover the arrival/replacement/cancel path in both v2 languages, mixed
imports and cache/cycles, sticky loading errors, rejected compiler forms and
poisoned ambient paths, JSON fidelity and safe rest, exact post-wait diagnostic
locations, runtime budget/signal/early-return cleanup, compiler operation,
expansion, memory, output and depth bounds, vendor hashes and plain Lua behavior.
Test-binary-only probes directly enter compiler C table/string/pattern paths to
verify the helper watchdog and joining; that injection is not in the shipped
helper or supported authoring subset.

`sigterm_during_compilation_joins_even_a_stopped_compiler` additionally inspects
the parent process tree, stops the actual compiler, sends SIGTERM to Due, and
checks both check/run cleanup. It is ignored by default because this host's
sandbox denied `/bin/ps` (`Operation not permitted`) on the attempted run. Run
explicitly where native process inspection is authorized:

```sh
cargo test --locked --offline --features workflow-fennel-spike --target-dir target/fennel \
  --test workflow_fennel sigterm_during_compilation -- --ignored
```

CI configurations include Fennel-only and the full comparison, but remote CI has
an account billing block; configuration is not evidence that remote jobs ran.
The outer caller owns independent matrix/probes, Claude review, real read-only
smoke, footprint measurements, sanitized publication and final recommendations.
The unrelated intermittent sibling-cancellation timeout in issue #19 remains
unresolved; a passing rerun does not repair it. No providers, delegation, issues,
PRs, commits, pushes, installation or other checkouts were used for implementation.

The supported surface demonstrates Lisp authoring conveniences over Lua, with
extra compiler isolation and diagnostic machinery. It adds no job capability and
does not repair Lua's JSON conventions, restricted C APIs or native cleanup limits.
No runtime winner, authoring-quality advantage, performance result or footprint
claim follows from passing these fake-child tests.
