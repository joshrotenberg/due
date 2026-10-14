# Stock compiler boundary experiment v0

The favorable result is a narrow explicit-builder library convention plus
better compile-error related locations using version-pinned plugin observations.
The negative result
is that these APIs do not supply definition-site binding identities, a complete
expansion stack, or a public runtime-PC-to-expansion identity mapping. No vendor
patch or compiler fork is included; the observation plugin itself is an unstable,
version-pinned dependency.

Run the full acceptance from the checkout:

```sh
# Rust 1.90+ on PATH; optionally set FINOCCHIO_TOOLCHAIN_BIN as in the README.
sh spikes/finocchio/test.sh
```

The crate-only bounded experiment is also reproducible with the same toolchain,
`TMPDIR=$PWD/target/finocchio-tmp`, `CARGO_TARGET_DIR=$PWD/target/finocchio` and
`cargo test --offline --locked --manifest-path spikes/finocchio/Cargo.toml --test compiler_boundary`.
Tests execute generated Lua in the separate runtime VM. All original acceptance
tests, vendor SHA256 and Due compiler/ownership checks remain in `test.sh`.

## API stability and upkeep

Fennel's pinned [API documentation](https://github.com/bakpakin/Fennel/blob/9af5e47fa7ddea806544d15507412ddc51bd046d/api.md#plugins)
explicitly excludes the entire plugin system from its normal backward-compatibility
guarantees. `assert-compile` is documented there; `macroexpand` is exposed by the
pinned compiler but is not enumerated in that hook list. Both require compatibility
tests on an upgrade. Calling a hook public below means exposed to a plugin, not
stable across Fennel versions. AST constructors, macro utilities and `ast-source`
are a separate API dependency from these hooks; private scope tables and the
second-return line-map layout add further implementation coupling.

The pinned [macro guide](https://github.com/bakpakin/Fennel/blob/9af5e47fa7ddea806544d15507412ddc51bd046d/macros.md#using-functions-from-a-module)
recommends loading a helper module into a gensym inside expansion. This spike has
no runtime module loader, so explicit dependency passing is a deliberate owned
convention. It does not establish a new general limitation on Fennel's module use.

Upgrade inventory: library macros and macro-cache setup; the plugin hook order,
arguments and AST identity behavior; separately the optional internal line map;
the optional internal `from-macro?` propagation hint; and the pre-existing semantic
analyzer's owned scope rules. The tests here exercise
the current pin, not another Fennel release. A version compatibility audit remains
a decision-plan gate.

## Entry point and evidence vocabulary

`Compiler::boundary_probe(source, filename)` returns `BoundaryProbe`:
`version: 0`, the unchanged `Compilation` shape, `events`, and `diagnostics`.
Only this entry point enables the extra in-memory `finocchio-boundary` module.
Ordinary compilation and the semantic subset do not admit `defn-with`.
The public macro cache is cleared for this module at each compile: searcher
restrictions alone do not stop an already cached module on a reused VM.

The observation plugin uses `macroexpand`, `assert-compile`, AST predicates and
`ast-source`. It returns nil from callbacks, never intercepts failure, never
reads compiler scope tables or mutates AST/source metadata. Its walker visits
list/sequence elements and map keys/values, treating symbols as leaves; it does
not traverse compiler annotations such as symbol referents. Budgets remain the
same; observation overhead can exhaust them sooner.

| Field | Meaning and limit |
|---|---|
| Event `id`, `input_node` | IDs local to this one probe. Node IDs identify actual Lua AST table objects, not matching printed text. Scalars have no object identity. |
| `location` | Snapshot of public `ast-source` fields: optional file/line/start/finish; zero-based UTF-8 bytes, end-exclusive. `origin=public-ast-source`, `confidence=reported`. Reported does not mean original caller source: the vendor may already have propagated metadata before callback. Optional `pinned_from_macro` snapshots the internal `from-macro?` marker; true records the vendor marker, while absence does not certify parser origin. The marker is set when a node filename differs from the call filename, including an absent filename; it does not prove a cross-file definition and can persist on input nodes of later expansions. This hint is an additional pinned dependency, not a stable source API. |
| Event `output_nodes` | Observed AST objects with role `root`/`descendant` and source snapshots. `retained-input` means the same object is in the input tree at this post-expansion callback; mutations before the hook are unobserved. `previous-output` means seen in an earlier output; `first-observed-output` means neither. These labels do **not** certify parser provenance or where a macro allocated/obtained a node. |
| Event `links` | All prior output appearances of this exact input object, with expansion ID, role, origin and `confidence=exact-identity`. They are evidence edges, not a unique causal parent/stack. Reused objects retain multiple edges. |
| Diagnostic | Public `assert-compile` message and AST identity/location; `phase=compile`, `origin=public-assert-compile`. `related` lists prior output appearances of that exact AST object. `attribution=related` if any exist, otherwise `unknown`. A retained caller node can relate to a macro without having been created by it. |
| Runtime attribution | Unknown through this probe. It emits no runtime mapping or invented runtime diagnostic. Tests inspect actual Lua errors separately. |
| `compilation.pinned_line_map` | Existing second-return layout of `compileString`, explicitly **internal/pinned**, not part of the public provenance claim. |

No nearest-line/text matching, synthetic failed-expansion events or overwriting
definition spans is used. No callback for a failed expansion implies no successful
event, not that no macro executed. Public callbacks are observations, not a log
of every compiler operation. Empty diagnostic arrays do not imply runtime safety.

## Evidence matrix

All named tests below are in `tests/compiler_boundary.rs` unless stated otherwise.

| Question | Executed evidence | Result / maintenance boundary |
|---|---|---|
| Generated free reference capture? | `generated_free_references_capture_and_existing_defn_rejects_shadows`: ordinary and nested locals, explicit macro-introduced local, `_ENV` each change the result of a generated `contract` lookup. Existing `defn` rejects each relevant shadow. | Public `in-scope?`/`assert-compile` support fail-closed name rejection. They do not bind the free reference to its definition site. |
| Stock binder protection? | Same test: quasiquoted `contract` binder fails “without gensym”; explicitly unquoting `(sym :contract)` can introduce it. | Useful accidental-capture guard; not full macro hygiene against deliberate symbol construction. |
| Explicit dependency practical? | `explicit_builder_is_once_lexical_recursive_and_interoperable`, `examples/boundary-bindings.fnl`: one counted builder-expression evaluation, checked recursion, host and higher-order Lua calls; adversarial `contract`, `_G`, `builder`, `builder_1_`, `_ENV` meanings survive. | Owned convention using public AST/gensym and ordinary functions. Existing `defn` is unchanged. |
| Does helper resolution follow the caller? | `explicit_builder_preserves_helper_and_environment_names_but_is_caller_authority`: nearest shadowed builder wins over poison outer builder; builder/function have same name; recursive bad argument fails; macro-introduced shadows work. Supplying an unchecked builder accepts a bad argument. | Caller-supplied authority, not definition-site binding or authentication. No generated global builder lookup. |
| `_ENV` means Lua spelling alone? | Same test compares checked and ordinary functions under nested `_ENV`. Stock Fennel mangles the nested parameter; both retain the enclosing global lookup behavior. The example's first `_ENV` parameter changes user global `type`. | Preserve actual stock Fennel meaning. Do not promise that every source `_ENV` binding becomes a fresh Lua environment. |
| Scope APIs sufficient for resolved bindings? | `public_scope_queries_observe_use_site_presence_not_definition_site_binding` uses `get-scope` only to observe a table and `in-scope?` only as a presence query at different use sites. Pinned source inspection below. | Exposed tables/manglings are not a stable resolved-symbol interface. No internal table fields used for supported output. |
| Is the extra module isolated? | `experimental_import_does_not_leak_through_the_public_macro_cache` alternates probe/ordinary/probe on one Compiler. Ordinary compile and semantic analysis reject the new import. | Small owned cache convention needed because cache precedes searchers; no ambient loader expansion. |
| Whole-output chaining? | `whole_output_chains_still_have_exact_identity_links`; original `tests/provenance.rs` retained. | Public success hook proves direct output/input object equality. |
| Descendants and repeated invocations? | `descendants_repeated_invocations_and_passthrough_have_distinct_identity_evidence`, `examples/boundary-provenance.fnl`: two `branch` calls emit nested `leaf` forms. | Descendant walking gives identity edges where the baseline has no parent. Calls have different identities despite identical generated definition spans. |
| Passed-through caller expressions and UTF-8? | Same test: `pass` retains `(leaf 30)` identity and full caller span. Source and a shifted multibyte prefix both execute to `[11,21,31]`. Generated `(leaf ,x)` has a start/definition line but **no end**. | Public spans are useful, incomplete, and preserved. No fabricated end offset. |
| Cross-file macro output? | `cross_file_library_expansions_report_propagated_spans_as_pinned_hints` invokes the real `defn-with` library twice, with and without a UTF-8 prefix, and executes both results. Generated roots carry complete caller spans and `pinned_from_macro=true`; input locations are unmarked. | Fennel replaces cross-file metadata before the hook. A complete reported span is not necessarily an original parser span. The marker is an internal hint, not exact runtime attribution. |
| Reused output ambiguity? | `reused_output_objects_keep_multiple_relations_instead_of_inventing_a_parent`: a macro returns the same cached AST twice; the second leaf has two prior output relations. Both Lua results are 3. | Identity proves reuse, not a unique parent. Keep all evidence; no forced attribution. Pinned source inspection also shows that a cross-file cached node can retain its first propagated call span when later reused within the same file; the reuse fixture itself is same-file. |
| Failed macro body vs generated compile error? | `compile_assertions_relate_only_observed_nodes_including_failed_macro_calls`: a generated unknown function relates by symbol identity to inner output; a throwing inner macro has no success event but its failing call AST relates to outer output. | Public `assert-compile` supplies useful related locations even without expansion-entry/exit hooks. Generated symbol may have only file/line; failed call may lack end. |
| Missing or misleading error evidence? | Same test: direct unknown identifier and direct throwing macro have no prior output relation (`unknown`); invalid passed-through caller expression relates with `retained-input`. | Related does not imply generated or caused by that macro. No inference from message text or source line. |
| Runtime failure at individual invocation? | `runtime_errors_execute_lua_but_public_events_do_not_identify_runtime_invocation`: compile two functions, each invokes the same error macro, then actually call both. Distinct actual Lua error PCs map via the internal map to the same macro-definition line. Public output roots have identical locations and different identities. | Runtime PCs lack public linkage to AST/event identities. The map supplies at most a related definition location. Tests do not infer invocation by matching error strings/order. |
| Limits/regressions? | `tests/boundary.rs::compiler_lua_loop_exhaustion_is_sticky` runs every original caught/nested-loop case through both APIs, in guarded/reaped children. `test.sh` retains vendor hash, all original tests and Due's no-user-macros test. | Same memory/input/instruction limits and production-adjacent restrictions. No security guarantee or hard C-call preemption. |

## Narrow missing interfaces

Pinned source inspected: `assets/vendor/fennel/fennel-1.6.1.lua`, SHA256
`c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2`.
These are implementation observations, not new public dependencies:

- `make_compiler_env` implements `get-scope` with `compiler.scopes.macro` and
  `in-scope?` with `scope.manglings[tostring(symbol)]` (around lines 2217–2232).
  `make_scope` constructs inherited mutable `manglings`, `symmeta`, `parent`, etc.
  (around 2644). Reading these fields would own their implementation coupling.
- `symbol_to_expression` invokes its plugin hook **before** resolution, then
  assigns `symbol.referent` from `scope.symmeta` (around 2900). Neither a hook
  exposing a scope nor that internal annotation is a stable API for constructing
  an expression bound to a particular lexical definition.
- `macroexpand_2a` calls the macro with `xpcall`, propagates metadata, calls
  `assert_compile`, and only then emits `macroexpand` (around 3261–3300).
  `assert_compile` exposes the failing AST (around 2665–2693). There is no
  paired public expansion-enter/exit/error observation here. Nested macro work
  done before a callback cannot be reconstructed as a full stack from success
  order alone.

For tooling, the narrow proposed binding API is an opaque resolved-binding
handle plus definition/reference source locations. For definition-site macros,
it also needs a supported way to construct a reference to that handle, with
scope/lifetime/environment rules the compiler actually honors. Merely returning
a generated Lua name, or placing a cached symbol into another scope, is not that
API. `gensym` solves *new* binders; explicit dependency passing solves this
library's selected helper. Neither resolves arbitrary free symbols in third-party
macros. There is no evidence here that a fork is necessary; adopting private
tables would incur version-specific maintenance, while an upstream API could
make the required contract explicit.

For provenance, the narrow proposed extension is paired expansion IDs with
entry/success/failure callbacks and explicit output-node origin relations that
can distinguish generated and retained nodes. A public emitted-Lua-span to AST
origin/expansion-ID map would additionally be needed for runtime PC attribution.
Runtime calls can repeat the same compiled function, so identifying a dynamic
execution instance would require runtime context as well. Owning a restricted
source/origin IR or instrumenting selected library forms are possible future
conventions, not results delivered here. Urn compiler passes and resolved
variables alone do not establish these runtime requirements.
