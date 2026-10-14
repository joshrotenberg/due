# Finocchio experimental language contract v0

Status: feasibility contract for this isolated spike, pinned to Fennel 1.6.1,
Lua 5.4 and mlua 0.12.2. **v0 is not a stability promise.** “Executable” below
means observed in this checkout; “selected” is the policy we choose for further
experiments; “deferred” is not implemented. The original probes retain their
boundaries; the [integrated module/workflow slice](integrated-workflow.md) now
combines this compiler, data library and runtime contracts with a public Due CLI
adapter. The semantic analyzer remains a separate, restricted tooling experiment.

## Executable behavior and evidence

Test paths below are relative to this spike except the explicitly named Due tests.

| Area | Executable v0 behavior | Test evidence |
|---|---|---|
| Native values | Ordinary Lua nil, booleans, strings, integer/float numbers, mutable tables and functions. Missing arguments become nil; nil removes a table entry. Array/map literals both produce tables. Tables compare by identity absent a custom metamethod; `1 = 1.0` numerically, but `math.type` distinguishes them. | `tests/language_contract.rs::native_tables_remain_mutable_identity_values_and_nil_removes_keys` |
| Immutable JSON | Every node is Rust userdata, including null and scalars. Missing `data.get` returns Lua nil; JSON null is present userdata. Empty array and map differ. Arrays use positive one-based integer keys; maps use strings. Nested access/iteration yields owned copies. No mutation methods; assignment, `rawset`, table insertion and metatable replacement fail. | `tests/data.rs::faithful_json_tags_nested_immutability_equality_and_iteration` |
| Numbers and equality | JSON parse/serialization retains arbitrary precision value representations, not original bytes. `data.scalar` admits i64 or finite floats whose serde_json number representation round-trips; `1e2`, `2.50`, huge integers and `1e999` remain data. Accepted floats still have binary rounding. JSON equality is structural serde_json equality, not universal mathematical equality (`1` vs `1.0` is not promised equal); `rawequal` is userdata identity. Duplicate object keys/order, whitespace and escape spelling are not preserved. | Same data test, plus implementation `src/data.rs`; no byte-preservation claim |
| Data projection | Use `data.kind/get/len/iter/scalar/json`. Native destructuring, array `match`, `pairs` and `#` do not implement the userdata protocol. | `tests/data.rs::native_table_patterns_and_destructuring_are_not_userdata_protocols` |
| Functions | Ordinary `fn` keeps Lua missing/extra argument and multiple-return semantics. Imported `defn` produces an ordinary function with exact positional arity, counting explicit/trailing nil. Predicates check arguments only, before implementation entry; `string?` accepts supplied nil, not an omitted slot. Recursive calls use the same checked wrapper; higher-order, Lua and Rust callers share checks. Multiple returns, including trailing nil, survive. Documentation is copied on retrieval. | Both contract tests `ordinary_recursive_higher_order_and_lua_calls_share_contracts`, `exact_nil_positions_multiple_returns_and_unknown_types`; `tests/language_contract.rs::native_fn_arity_and_checked_trailing_nil_multiple_returns` |
| Bindings/macros | Ordinary lexical bindings follow stock Fennel lowering. Existing `defn` requires an unshadowed global `contract` and environment: it rejects surrounding/own `contract` or `_ENV` bindings. `gensym` protects introduced temporary names; generated free references can still capture. Quoted non-gensym binders are rejected, but explicitly constructed symbols can introduce capturing locals. | `tests/contracts.rs::defn_rejects_contract_capture_without_introducing_a_global_table_capture`; binding tests in `tests/compiler_boundary.rs` |
| Explicit dependency experiment | Only `Compiler::boundary_probe` enables `finocchio-boundary/defn-with`. Caller supplies a builder expression; a gensym binds it once at declaration. With `contract.define`, recursion/interop/checks work under caller shadows, including macro-introduced names. A substituted unchecked builder bypasses checks. This is caller authority, not definition-site binding or arbitrary macro hygiene. | `explicit_builder_is_once_lexical_recursive_and_interoperable`, `explicit_builder_preserves_helper_and_environment_names_but_is_caller_authority`, `experimental_import_does_not_leak_through_the_public_macro_cache` |
| Modules/exports | The semantic tool accepts static acyclic logical modules, literal `require`, explicit string export keys and known members; rename preserves export keys. It rejects cycles, dynamic lookups, shorthand exports and unknown constructs. The standalone runtime has **no loader**. Semantic execution tests inject a narrow test `require`; that does not ship a module system. Compiler macro search is fixed/in-memory. | `tests/semantic.rs` cross-module, bad-calls and shorthand tests; `tests/boundary.rs::compiler_and_runtime_capabilities_are_separate_and_no_ambient_loader_exists` |
| Errors/ownership | Contract failures are Lua errors; compiler failures are separate from generated-Lua runtime failures. Ordinary protected calls catch ordinary errors. Compiler instruction exhaustion is sticky, including caught/nested loops. Actual job ownership, cancellation, join and cross-scope rejection are tested separately through the existing Due Rust host, not this runtime. | `tests/boundary.rs`; `tests/compiler_boundary.rs` compile/runtime failure tests; root `tests/workflow_composition.rs::finocchio` (unchanged) |

## Selected policy

Keep native and immutable values explicit; do not silently coerce JSON null into
nil, immutable arrays into tables, or unrepresentable numbers into floats. Keep
checked declarations opt-in. Existing predicates are `number`, Lua-subtype
`integer`, `string`, `boolean`, `function`, `nil`, `any`, `string?`; neither
positivity, finiteness nor return types are checked. Runtime builder trust must
be explicit if the dependency convention is pursued.

Keep semantic operations fail-closed on their documented subset. This is an
owned scope IR, not compiler-backed general type checking. Static modules and
stable explicit export keys are the selected tooling policy. The integrated
entry point now has an explicit configured loader; broader deployment and
runtime bundle distribution semantics remain deferred.

Spans use **zero-based UTF-8 byte offsets, end-exclusive**, with one-based lines.
Semantic diagnostics currently use `file,line,start,finish,message`; legacy
expansions use `start,end`. The opt-in probe uses `location` with
`file,line,start,finish,origin,confidence`, retaining absent fields. Its optional
`pinned_from_macro` field is a version-specific vendor propagation hint, not proof
of exact origin; complete spans can already have been rewritten by Fennel. It adds
compile assertion diagnostics with `phase,message,node,related,attribution`.
`exact-identity` concerns AST object identity only; `related` is not exact
runtime causality. `unknown` and incomplete spans must remain visible. Never
replace definition spans with invocation spans or infer parents from text/lines.
See [compiler-boundary.md](compiler-boundary.md) and the provenance tests.
Compiler/parser errors without callback evidence remain strings; CLI IO,
allocation and budget failures remain stderr/host errors. A unified structured
error envelope is proposed, not implemented.

Capabilities belong to the owning host. Compilation uses a separate VM, pure
libraries and public AST APIs, no IO/process/network/agent/runtime capabilities.
Runtime tests have no package/OS/IO/debug or ambient loaders. Budgets remain
128 KiB compiler input, 32 MiB Lua allocation, 10 million cooperative instructions
per Compiler after initialization; exhausted instances stay exhausted. Data
parse is limited to 1 MiB and serde_json's nesting limit; Rust data allocations
are separate. Macro code can affect compiler state, C-library calls lack hard
preemption, and runtime tests execute trusted Lua. These are bounded experiments,
**not universal sandboxing**. Production-adjacent Due compiler restrictions stay
unchanged and are checked by `test.sh`.

## Integrated extension

`workflow run/describe/check` uses the same compiler, immutable data and runtime
contracts with an explicit host-configured module loader and Due CLI adapter.
The table above describes original standalone probes; its statements about no
runtime loader and separate async ownership do not describe this new entry point.
`data.pack/object/array` add explicit owned snapshot construction, with ambiguous
empty native tables, cycles and non-JSON values rejected. Integrated JSON
boundaries add depth 32 and 1 MiB limits. Loading may execute pure initializers
but never admits agents. The integrated VM is deliberately narrower than
`runtime()`, including selected C-loop controls and sticky cooperative yielding.
See [the complete integrated contract and limits](integrated-workflow.md).

## Deferred surface

No resolved-symbol API, fully hygienic third-party macros, complete expansion
entry/exit stack, exact runtime-to-invocation mapping, general LSP/type system,
new scheduler or general immutable-data pattern system is delivered. Urn's
resolved variables/syntax quote/pattern views are inspiration, not dependencies
or evidence that runtime provenance is solved. Finocchio may inform an
agent/harness toolkit using Due's Rust provider runner; it is not a product
selection. BEAM supervision, distribution and durability are independent
requirements, not consequences of choosing Lua.
