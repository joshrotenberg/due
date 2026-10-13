# Finocchio feasibility spike

Stock **Fennel 1.6.1 + mlua 0.12.2 + Lua 5.4** can supply useful declaration
macros, immutable data and Rust-owned async composition without a compiler fork.
Semantic tooling needs its own analyzed representation; precise nested macro
source mapping remains a partial outcome. This is an experiment, not a release.

Run all five probes and the compiler-boundary regression, from the checkout:

```sh
sh spikes/finocchio/test.sh
```

Use Rust 1.90 or newer on `PATH` (or set `FINOCCHIO_TOOLCHAIN_BIN` to its bin
directory). The script runs Cargo offline/locked,
checks the vendor SHA256, and keeps targets and temporary files under `target/`.
It runs 19 standalone tests, three Due ownership tests (both signals in one),
and the existing no-user-macros regression. No model/provider invocation occurs.
The native protocol fixture is a local shell program, not an agent.
The ownership checks require a Unix target and a working native toolchain. The
script fails on missing/ignored filtered tests rather than accepting a zero-test
pass. SHA256 verification uses `shasum` or, if absent, `sha256sum`.

This crate has its own `[workspace]` and lockfile. The only existing source file
changed is `tests/workflow_composition.rs`, adding a gated `finocchio` module.
Due's runtime, root Cargo manifest/lock, feature defaults, installed CLI and
restricted compiler are unchanged. The async probe uses the existing Due host
directly; there is no standalone fake-job scheduler whose cleanup is substituted
for Due evidence.

| Probe | Demonstrated capability / evidence | Deliberate limit | API / coupling |
|---|---|---|---|
| Contracts | `examples/contracts.fnl`, `tests/contracts.rs`: ordinary Fennel calls, recursion through the checked function, higher-order calls, direct Lua/Rust calls, exact arity (including trailing nil), positional errors, copied machine-readable docs | A library declaration, not altered Fennel calls. Argument predicates only; no return/dependent/static type system | Public macro import/searcher, quasiquote/AST constructors and ordinary Lua closures |
| Data | `examples/data.fnl`, `tests/data.rs`: null/empty array/empty map, nested immutable Rust userdata, structural equality, explicit iteration/get/tag matching, JSON numbers beyond Lua's range | Table destructuring raises an indexing error; ordinary array `match` takes its fallback; `pairs` iteration and `#` fail. Use `data.*` | Public mlua userdata/metamethods and serde_json; no read-only proxy tables |
| Provenance | `examples/provenance.fnl`, `tests/provenance.rs`: actual expansion input/output, original call span, direct parent edges from AST identity, failing inner macro and generated runtime error | Successful-expansion hook omits an event for a macro that throws. Same-file generated forms retain definition lines. No complete expansion stack or exact reverse mapping | Public `macroexpand` plugin and `ast-source`; separately labelled `compileString` second-return map has pinned **internal layout** |
| Semantic tool | `examples/semantic/`, `tests/semantic.rs`: definitions/references, bad contracted calls, capture-checked binding rename, shadowing, UTF-8 offsets, untouched strings/comments, aliases and two supported macros | Closed subset below; unknown forms/macros return unsupported with no edits. Explicit export keys stay stable. No general type inference or arbitrary macro analysis | Public parser, AST predicates, `ast-source`, `syntax`; **own scope IR** in `lua/semantic.lua`, no compiler scope internals or handwritten Fennel parser |
| Async ownership | `examples/async/`, `tests/workflow_composition.rs::finocchio`: two copies of a Fennel component suspend through a Lua wrapper into Rust; independent barriers prove overlap; SIGINT/TERM join active/queued work; cross-scope handles fail even under `pcall` | This probes existing Due structured ownership, not new language syntax or production agents. No general cancellation guarantee for arbitrary Lua/native code | Existing Due `start/wait/concurrent` API and supervisor; unchanged pinned Due Fennel adapter |

## Declaration semantics

```fennel
(import-macros {: defn} :finocchio)
(defn greeting [name :string?] "Explicit nil means anonymous."
  (or name :anonymous))
(greeting nil) ; "anonymous"
(greeting)     ; contract greeting: expected 1 arguments, got 0
```

`defn` expands to a local binding with a closure builder. Its generated
`contract.define` reference requires an unshadowed global `contract`: the macro
uses public `in-scope?`/`assert-compile` to reject an enclosing local or parameter
with that name, and also rejects `contract` as its own name or parameter. It does
not use `_G` as a workaround; a local/parameter `_G` remains safe. The semantic
subset conservatively reserves `contract` for all bindings and rename targets,
even in modules without `defn`. This is rejection of capture, not general hygiene
for arbitrary generated free references.

Recursion receives the
public checked function, so recursive calls cannot bypass checks. The resulting
value is an ordinary Lua function: it can be passed to another function or called
from Lua. `contract.describe(f)` returns `{name, parameters, doc, arity, semantics}`
before the first call; mutating this copy does not change the checks. Multiple
return values are preserved.

Predicates: `number`, `integer` (Lua integer subtype, so `1.0` fails), `string`,
`boolean`, `function`, `nil`, `any`, `string?`. The last accepts nil or a string,
but still requires a supplied argument. Missing and extra arguments fail before
type checks; `select('#', ...)` counts explicit/trailing nil. Predicates do not
enforce positivity, finiteness, or a return contract. Ordinary `fn` retains Lua's
missing/extra argument behavior; the semantic tool diagnoses arity only for
contracted declarations.

## Data semantics

```fennel
(local value (data.parse "{\"items\":[null,{},[]]}"))
(local items (data.get value :items))
(each [index child (data.iter items)]
  (assert (= (type child) :userdata)))
(match (data.kind items) :array (data.len items) _ 0) ; 3
```

Every node, including scalar/null nodes, is owned userdata. `data.get` accepts a
string for a map or a positive, one-based integer for an array; a missing entry
returns Lua nil, while JSON null remains tagged userdata. `data.scalar` unwraps
strings, booleans, i64 integers and finite floats whose `serde_json` serialization
matches the stored number spelling. This is a conservative spelling check, not
exact decimal-value comparison: `1e2` and `2.50` remain data but cannot unwrap,
while `100.0` and `2.5` can. Accepted floats still have ordinary binary floating
point semantics (for example, `0.1` is not exactly representable in binary).
Out-of-range and spelling-changing conversions fail. Keep these numbers
as data and use `data.json`. There are no mutation methods or writable nested
tables. Getting/iterating children creates owned snapshots (copying, not a
persistent-data-structure performance claim).

`=` compares JSON structure and serde_json number representations; `rawequal`
still tests userdata identity. With `arbitrary_precision`, very large integers
and long decimal values survive parse/JSON conversion; `1e999` can remain data
but cannot become a Lua float. Number spellings may normalize and `1` and `1.0`
are not promised mathematical equality. Object key order, whitespace, escape
spelling and duplicate keys are not preserved (last duplicate wins). Faithfulness
means JSON value semantics within these rules, not byte-for-byte source text.
Parsing has a 1 MiB input limit and serde_json's default nesting limit.

## Provenance observations

Expansion `start`/`end` use the same **zero-based UTF-8 byte, end-exclusive**
convention as semantic spans (`start`/`finish`). Both ends are tested against the
source, including a multibyte prefix. Lines are one-based. Generated AST nodes
can lack span fields or retain macro-definition spans; these are not invented
user callsites.

The shipped negative example has the outer user call on line **5**. The public
hook records `(outer 42) -> (inner 42)` at that exact parser byte position, then
`(inner 42) -> (not-a-real-function 42)`. The second event's `parent` is the first
event's ID because its input is the actual prior output AST object. Its line is
**3**, however, and the final unknown-identifier diagnostic points to the inner
macro definition on line **2**. Shifting the source by two lines shifts the
observations by two; these are compiler observations, not seeded filenames.

A separate runtime-error test identifies the real generated Lua error line and
looks it up in `pinned_line_map`. That tuple points to the inner definition, not
the outer user call. The second-return map layout is implementation-dependent
even though `compileString` itself is public. Using it does not recover an exact
nested source map. Richer tooling would need retained provenance/source IR or
deeper version-specific instrumentation; this spike does not establish that a
compiler fork is necessary. Only direct output-to-next-input identity edges are
tracked, not every descendant generated AST or repeated runtime invocation.

## Semantic interface

The CLI takes a JSON object mapping logical module names to Fennel source strings.
It prints JSON, returns nonzero on unsupported input/diagnostics, and never writes
rename edits. Offsets are **zero-based UTF-8 byte offsets**, ends exclusive.

```sh
# Use a Rust 1.90+ toolchain on PATH.
export CARGO_TARGET_DIR="$PWD/target/finocchio"
export TMPDIR="$PWD/target/finocchio-tmp"
mkdir -p "$TMPDIR"
python3 -c 'import json,pathlib; p=pathlib.Path("spikes/finocchio/examples/semantic"); print(json.dumps({n:p.joinpath(n+".fnl").read_text() for n in ["main","math"]}))' > target/finocchio-sources.json
cargo run --offline --locked --manifest-path spikes/finocchio/Cargo.toml -- check target/finocchio-sources.json
# Offset 49 selects the member "bump" in main's alias.bump call.
target/finocchio/debug/finocchio-spike definition target/finocchio-sources.json main 49
target/finocchio/debug/finocchio-spike rename target/finocchio-sources.json main 49 raise
# Intentional compile failure, with expansion evidence (exit 1):
target/finocchio/debug/finocchio-spike provenance spikes/finocchio/examples/provenance.fnl
```

`compile FILE` also returns generated Lua plus the evidence fields. Tests execute
that Lua in a separate runtime VM. The CLI does not execute the generated runtime
Lua. `compile` and `provenance` do execute user macro bodies in the compiler VM,
with the resource limits and qualifications below.

Supported source subset: simple `local`, named/anonymous `fn` with fixed simple
parameters, sequential `let`, `do`, three-argument `if`, literal sequences/maps,
basic arithmetic/comparisons/concatenation, known-shape function calls, static
acyclic `require`, explicit string export keys and one-level module member access.
Only unaliased imports of the shipped `defn` and `twice` macros are analyzed.
The latter evaluates its argument once using `gensym`; the scope IR records the
source expression and never invents editable positions for generated symbols.
Unsupported: mutation, destructuring bindings, arbitrary macros, quoted code,
dynamic module/member lookup, shorthand map/export entries such as `{: x}`,
varargs, callbacks of unknown shape, declarations
in expression positions, nested macro imports, module cycles. The tool does not
execute user macros. It is not an LSP or a general type checker.

The parser normalizes shorthand to the same map as explicit keys, including in
its private `keys` metadata. To avoid changing an implicit key during rename,
the analyzer reparses each map's original contents as a sequence with the public
parser, then rejects standalone `:` keys. Comments and whitespace are handled by
that parser, not textual substitution. Macro-import destructuring `{: defn}` is
handled separately and remains supported. This adds parsing work (nested maps
can be reparsed repeatedly); the own scope IR also duplicates the supported
forms' binding rules and reanalyzes the bundle after every rename. There is no
claim of compiler-equivalent analysis, incremental performance or zero upkeep.

Rename resolves a source binding, edits only validated parser spans, then
reanalyzes the whole bundle and checks every reference's binding identity. A
capture, duplicate, unknown construct, or reserved name rejects the candidate.
Renaming an exported function changes its local definition/body/export **value**;
the explicit export **key** stays stable, so `alias.bump` continues working.
Selecting that member resolves to the original definition in the other module.
Renaming a module alias edits its own binding and member-access prefixes. Tests
compile and run both original and renamed modules to corroborate the model.

JSON result fields (arrays remain arrays when empty; internal value/type/export
inference state is not exposed):

| Result | Fields |
|---|---|
| Successful `check` | `supported: true`, `definitions: []`, `references: []`, `diagnostics: []` |
| Definition entry | `id` (one-based, scoped to this analysis), `name`, `file`, `start`, `finish` |
| Reference entry | `binding` (definition ID), `file`, `start`, `finish`, `editable` (false for stable export-key members) |
| Diagnostic entry | `file`, `line` (one-based), `start`, `finish` (the whole call), `message` |
| Successful `definition` | `supported: true`, `definition` entry, `references: []` for that binding |
| Successful `rename` | `supported: true`, `sources` (complete candidate bundle), `edits: []` (original definition/reference entries in descending offset order per file), `diagnostics: []` from reanalysis; the replacement is the requested name |
| Unsupported semantic operation | `supported: false`, `error`; no partial definitions, sources or edits |
| `compile` / `provenance` | `lua` or `error` (the other is null), `expansions: []`, `pinned_line_map: []` |
| Expansion entry | `id`, nullable `parent`, `name`, nullable `file`, `line`, `start`, `end`, plus rendered AST strings `input` and `output` |
| Pinned line-map entry | `[generated Lua line, source filename, source line]`; lines are one-based; pinned internal layout |

Diagnostics do not set `supported` to false: that flag describes the analyzed
subset. The CLI still exits nonzero on diagnostics or compilation errors.
Argument, input IO, VM allocation and instruction-budget errors are host failures
reported on stderr with nonzero exit, rather than a JSON result.

## Async evidence and trust boundary

Neither component can finish before its release file exists. The test observes
both readiness barriers before releasing either, then releases B and verifies A
is still unsettled. Owner scopes are distinct, results preserve input order,
receipts agree with ownership, and each non-root owner closes after its jobs
settle. Cancellation observes two active jobs plus a third admitted/queued job;
active exits are 130 and the queued job has `started=false`, `code=null`. Recorded
native PIDs are checked for cleanup. A caught cross-owner handle error cancels
the original owner's active work and prevents post-error admission. Dropping a
future alone is not counted as cleanup evidence.

User macros execute code. The standalone compiler has a separate VM with pure
libraries and AST APIs, a fixed in-memory macro searcher, and no filesystem,
process, network, agent or runtime host capability. The vendor's private preload
loader resolves embedded modules only. Compiler loading and the read-only
`debug.traceback` bridge remain host-private; neither is supplied in `compilerEnv`.
The runtime has no package library, require/load/loadfile/dofile, OS, IO or debug.
Tests verify these boundaries and sticky Lua-loop instruction exhaustion,
including caught and nested caught loops and aliases of the compiler's `pcall`.
Every compiler `pcall`/`xpcall` is wrapped before vendor loading; Rust checks the
exhaustion flag before and after each protected call, including vendor-internal
ones. Ordinary Lua hook errors are catchable. These wrappers re-raise exhaustion
while letting ordinary errors and multiple return values retain their behavior.
The loop tests run in subprocesses with a five-second failure guard that kills
and reaps only the test's compiler child. This is test containment, not a new Due
runtime subprocess mechanism.

This is **not a sandbox-security claim**: compilation is in-process, compiler
macros can alter compiler state, C-library work has no hard wall-time preemption,
and runtime execution here is trusted test code. Compiler input is bounded at
128 KiB, VM allocation at 32 MiB, and a cooperative Lua instruction budget of
10 million per Compiler after vendor initialization, sampled every 1000
instructions. An exhausted Compiler rejects further compilation;
Rust-owned JSON allocations are separate. Due's more restrictive compiler stays
unchanged and its macro-rejection regression runs in the acceptance command.

The unmodified compiler is included from
`../../assets/vendor/fennel/fennel-1.6.1.lua`, SHA256
`c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2`.
See that directory's README and LICENSE for upstream commit/provenance. This
implementation verifies the byte hash, not a new upstream signature check.
