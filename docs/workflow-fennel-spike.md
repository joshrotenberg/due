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
compiler entirely, including when the feature is present; plain Lua/Rhai checks
also retain normal OS signal handling without registering compiler handlers.
`.lua` and `.fnl` modules share manifest-only require, cache, cycle detection,
effect-disabled initialization and plain string-keyed function exports (at most 128). Module
load failures, undeclared imports and cycles are sticky through pcall for v2
and bundles containing Fennel. Plain v1 Lua retains recoverable ordinary module
errors. Effects attempted during module loading remain sticky in every version.
No child can be admitted before v2 invocation.
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
integral floats fail. Tests preserve large Lua/Fennel integer literals including
9007199254740993 and i64 max. Use input or math.mininteger/maxinteger for signed
i64 endpoints, avoiding parser differences at the minimum decimal literal.

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
including when the parent future is dropped. The helper stays in the parent's
process group. Helper SIGINT/SIGTERM exits explicitly mark compilation interrupted
(cancelled/130), even if helper completion wins the parent signal race; other
abnormal exits remain compiler failures. There is no offloaded compiler thread
to abandon. Source, generated-output and map checks also bound Rust-side
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

Compilation uses actual manifest source names, including nested paths. Generated
Lua chunks append ` (generated Lua)` to those names, so a raw generated line
cannot masquerade as a mapped original Fennel line. Successful snapshots retain
original Fennel, generated Lua, compiler version/hash/upstream commit/URL, fixed
options, limits, the integration driver, and generated-line mappings. The compiler
library is unmodified; provenance and
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
available, matching complete chunk names rather than path suffixes. Only bundles
containing Fennel enable the extra host-only line hook; plain Lua retains its
instruction-count hook without per-line location locking or formatting. The line
hook retains the last workflow file/line, including a tail-called callback after
real due.wait suspension and instruction-hook yields, where mlua's callback traceback loses its script frame. The error text
labels that context as **last workflow location**, not necessarily the origin
of an independent native collector failure. Debug is never exposed to scripts.
Some generated lines have no map; their raw generated location remains useful
alongside the snapshotted output. Arbitrary user error text is not trusted as
proof of a source location; post-wait tests assert the exact mapped form against
the snapshotted map and require different generated and original line numbers.
Lua may truncate long chunk names in raw errors, with the generated marker
using some of that space. The host line hook retains the full path: an independent
long-path probe verified the original line alongside the truncated raw error.
Mapping arbitrary message text remains heuristic; it is not source provenance.

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

`signals_during_compilation_join_even_a_stopped_compiler` additionally inspects
the parent process tree, stops the actual compiler, and tests SIGINT/SIGTERM
to the parent, the isolated workflow process group and the helper alone, checking
both check/run cancellation and cleanup. It verifies that the helper inherits
the workflow parent's group. It is ignored by default because this host's
sandbox denied `/bin/ps` (`Operation not permitted`) on the attempted run. Run
explicitly where native process inspection is authorized:

```sh
cargo test --locked --offline --features workflow-fennel-spike --target-dir target/fennel \
  --test workflow_fennel signals_during_compilation -- --ignored
```

## Caller acceptance results

Codex implemented `9baafe3` and its review repairs through stable Due. Claude
independently reviewed frozen snapshots through Due, approving the final changes.
The caller verified the source and ran the post-repair matrix below on macOS arm64
with explicit Cargo/rustc **1.90.0**. Main and installed Due were not modified.

| Feature selection | Passing tests | Ignored by the ordinary suite |
| --- | ---: | ---: |
| Default | 114 | 0 |
| Rhai | 146 | 0 |
| Lua | 136 | 0 |
| Lua + Fennel | 157 | 1 |
| Full comparison | 178 | 1 |

All five Clippy configurations passed with warnings denied, along with current
rustfmt, whitespace checks and 22 Node caller tests. The expanded ignored compiler
process-inspection test above **passed separately** with caller-side permission,
checking parent/group/helper signals in both check and run. The worker's earlier
EPERM was an environment boundary, not a reproduced compiler-lifecycle failure.
A subsequent small filename-with-spaces guard passed its augmented unit test,
Fennel Clippy and two independent release-binary diagnostic cases; no job logic
changed after the matrix.

The caller's separate fake-native fixture passed **44/44** cases. It verifies
arrival-order branching and replacement, early return/error/budget/signal cleanup,
mixed modules and sticky initialization, explicit null/empty containers, exact
integer literals, rejected cyclic/mixed/sparse values, compiler restrictions and
precise nested `.fnl:6` diagnostics after a real asynchronous wait. An initial
caller validator incorrectly assumed every cancelled child's raw stdout was
valid JSON; it was corrected to require that only when a decoded child result
exists. Empty/partial cancellation captures are retained, not invented as results.

A [two-provider authoring trial](https://github.com/joshrotenberg/due/issues/22)
used one prompt/reference/schema. Claude and Codex each wrote both languages;
all four scripts ran unchanged in four scenarios, **16/16 passed**. The independently
selected outcome is a tie. Model/effort defaults were omitted and observed values
remain unknown. This is no evidence of a general agent-authoring advantage.

The pre-repair release Fennel build (`9baafe3`) also ran a real read-only workflow: concurrent Claude
and Codex reviews of a synthetic fixture, then Claude synthesis. All three native
envelopes, answer contracts and traces validated (16, 14 and 16 trace records,
contiguous with successful terminals and no drops). Saved native receipts were
preserved, the final answer retained the known defect, and workspace hashes did
not change. No arbitrary provider deadline was imposed; the caller monitored
progress. The subsequent fixes change interruption reporting, legacy module error
handling and diagnostics. They were checked with the full post-repair matrix and
fake-native probes; the paid happy-path calls were not repeated.

## Review repairs and independently reproduced failures

- Group-delivered SIGINT/SIGTERM could produce failed/1 instead of cancelled/130
  when helper exit won the select race. The caller reproduced **30/32** wrong
  statuses while all helpers were still cleaned up. Explicit interrupted-exit
  classification passed the same **32/32** stressed cases, without moving the
  helper out of the parent's group or masking other compiler crashes.
- Plain v1 Lua inadvertently lost ordinary module-error recovery. Four independent
  before/after cases cover missing modules, bad exports, initializer errors and
  cycles. Recovery is restored only for plain v1; v2/Fennel failures and all Due
  effects during initialization remain sticky.
- Overlapping filenames could produce spurious source mappings. Generated chunks
  now carry an explicit marker; matching prefers complete known filenames and
  handles spaces/colons as well as slash-separated paths. Tests distinguish the
  exact original line from generated lines and verify the saved map. The caller's
  **10/10** compatibility/diagnostic cases and final space/colon cases passed.
- Plain Lua/Rhai checks no longer install Fennel compiler signal handlers. Extra
  per-line location tracking runs only in bundles containing Fennel. The documented
  v1 Lua entry importing a Fennel component now has a regression test.
- A superseded test harness hung because Rust consumed an externally sent SIGSEGV.
  The worker disclosed the remaining test child; caller process inspection found
  and terminated that exact orphan and verified its absence. The corrected
  test-only probe restores the OS disposition and bounds waiting with kill/join
  cleanup. This was a test-harness failure, not a production compiler leak.

Claude's focused repair review approved the changes. A final small review also
approved the filename guard and test-only cleanup repair with no findings.
Long-path truncation above remains a documented diagnostic limitation.

## Incremental footprint and assessment

Local release builds from the same source/compiler/profile, without stripping:

| Build | Executable bytes | MiB | Unique normal dependency entries, including Due |
| --- | ---: | ---: | ---: |
| Default | 3,147,920 | 3.002 | 61 |
| Rhai | 12,092,224 | 11.532 | 119 |
| Lua | 9,917,344 | 9.458 | 121 |
| Lua + Fennel | 10,362,112 | 9.882 | 121 |
| Full comparison | 13,294,784 | 12.679 | 132 |

Fennel adds **444,768 bytes (about 434 KiB)** over the Lua build and no additional
normal Rust dependencies. Vendored compiler source, license and compiler-binding
maintenance still count as added dependencies/responsibilities. Normal dependency
counts exclude build dependencies; cached build times are not comparable.
`otool -L` shows only system libraries. These are local footprint measurements,
not portable promises or interpreter-speed results.

Fennel's tested benefit is authoring: destructuring, match, comprehensions,
threading and Lisp syntax over ordinary Lua functions. It adds no host job
capability and does not repair Lua's table/JSON conventions, native build,
restricted C APIs or native cleanup limits. It adds a compiler subprocess and
pinned implementation-dependent source-map integration. User macro libraries
are excluded here, so this experiment establishes no broader ecosystem benefit.

The Rhai-versus-Lua decision remains open. The previous narrower recommendation
in [the Lua comparison](workflow-language-comparison.md) is not overturned by
Fennel syntax; if Lua is chosen, Fennel is a viable optional authoring front end.
Combined runtime features exist for experiments, not a shipping recommendation.
Keep the job lifecycle and outcome truth in Rust and composition policy in scripts.

The unrelated intermittent sibling-cancellation timeout in
[#19](https://github.com/joshrotenberg/due/issues/19) remains unresolved; passing
this batch does not establish a repair. Remote
[CI](https://github.com/joshrotenberg/due/actions/runs/38312111499) was rejected
before any steps ran because of GitHub's account payment/spending-limit check.
Local green checks are not presented as remote or cross-platform CI evidence.
