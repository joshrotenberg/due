# Squint over the QuickJS spike

Squint is a viable optional authoring frontend for the existing JavaScript
workflow API. A pinned Squint 0.15.212 proof ran unchanged QuickJS/Rust code:

```text
Squint .cljs source → external compiler → JavaScript + Squint core library
                                                   ↓
                                       QuickJS → Due's Rust supervisor
```

This is the same architectural role as Fennel over Lua: a second authoring
language over one runtime and host API. It adds Clojure-style syntax and library
functions, not another agent lifecycle or scheduling model. The current proof
precompiles externally; Due does not yet accept `.cljs` files directly.

## Working example

The [source example](../examples/workflow/squint/README.md) contains a reusable
design → implement → review pipeline, two concurrent instances, a custom
JavaScript review function, and a synthesis step. Its entry has the same two
arguments and JSON result contract as JavaScript:

```clojure
(defn ^:async main [due input]
  (await (.concurrent due "reviews"
           [(fn [child]
              (.run child {:id "a" :profile "claude" :prompt (:task input)}))
            (fn [child]
              (.run child {:id "b" :profile "codex" :prompt (:task input)}))])))

(def default main)
```

The compiler emits native async functions and promises. Scope-bound capabilities
are passed explicitly through lexical closures, just as in the JS adapter.
Profiles and permission grants remain caller-owned manifest data. No additional
Rust bindings or agent-running wrapper are needed.

## What was verified

Local synthetic-provider checks on 2026-10-28 used the previously tested release
binary from `spike/quickjs-workflows`, without rebuilding or modifying Rust:

| Case | Observed result |
| --- | --- |
| Squint entry + two Squint pipelines + JS custom review + synthesis | 7 successful runs; 7 saved receipts; sorted/materialized findings |
| Unchanged JS entry importing the Squint pipeline | 7 successful runs with validated answers |
| Plain maps/vectors/nil/keywords and `mapv`, `filter` + `vec`, `sort`, `assoc` | Expected JSON preserved |
| Returning a lazy `map` value | Rejected by the existing JSON boundary |
| Expected native failure | Inspectable `ok: false` outcome; caller handled it and completed |
| Error after awaiting a native run | Original message retained; location points to generated `.js` |
| Parent cancellation with two active synthetic native runs | Exit 130; cancelled receipt; both children and their sleep descendants joined |

The checked-in build helper was then run separately and its generated example
passed both `workflow check` and `workflow run`. Source maps named `main.cljs`
and `pipeline.cljs` were emitted and inspected. These are local compatibility
checks, not a claim of complete Squint support, real-provider verification, or
better agent authoring performance. Hosted CI remains blocked by the repository's
billing/spending limit; no hosted Squint validation is claimed.

## Packaging and semantics

Squint's package includes a compiler and a separate standard library. This
proof copies the published, unmodified `src/squint/core.js` (128,119 bytes) into
the workflow bundle. The workflow needs that library but not the compiler or
Node at runtime. Other Squint libraries would need their own captured imports;
there is no automatic package resolution.

The spike only accepts manifest import names under `local/`. The build helper
therefore rewrites the exact generated core import header to `local/squint-core`
and emits `.js` files. It guards that header against compiler output changes.
The pinned compiler's plain JavaScript `import-maps` option did not remap that
core specifier in the proof; this small packaging adaptation keeps Rust unchanged.

Maps and vectors are JavaScript objects and arrays; keywords become strings and
nil becomes null. These are mutable JS data structures with shallow-copy library
operations, not Clojure's persistent collections. Lazy sequences, sets, custom
classes, and other non-JSON values must be converted before crossing the host
boundary. Materializing an outer collection does not convert nested lazy values.
The JS adapter's safe-integer and other value limits still apply.

Functional collection helpers used by the example load and work under the
current frozen-prototype restrictions. Features that extend built-in prototypes
are outside that surface. Printing helpers may need a console that the harness
does not provide; workflow logging should use the existing `due.log` capability.
Ordinary JavaScript interop does not confer Node filesystem, process, timer, or
network APIs.

Keep the `due` capability explicit. Squint dynamic vars are shared mutable boxes,
and `binding` saves/restores values with `finally`; that is not async-local
context. A global `*due*` would reintroduce the scope attribution problem the
QuickJS adapter deliberately avoids.

## Remaining work before direct source support

The largest missing integration is diagnostic mapping. Squint emits v3 source
maps, but Due currently reports generated-JS locations. A direct-source frontend
should preserve useful `.cljs` filenames and locations across awaits and module
boundaries before being treated as equivalent to the Fennel frontend.

Embedding the compiler is a separate bounded experiment. Squint separates its
browser-oriented compiler API from its Node/macro-loading path. The latter uses
SCI plus ambient filesystem/module capabilities and unrestricted access; it
belongs to trusted build work, not the current restricted runtime surface. Any
embedded compiler and macro support would need explicit limits and capabilities.
This proof neither embeds the compiler nor enables custom macro loading.

The useful next choice is whether external precompilation is enough for the
intended agent experience. The runtime compatibility evidence is already good;
another execution engine or a wider Due host API is unnecessary for these cases.

## Upstream references

- [Squint language semantics and async functions](https://github.com/squint-cljs/squint#differences-with-clojurescript)
- [Compiler and standard library](https://github.com/squint-cljs/squint)
- [Source-map API](https://github.com/squint-cljs/squint/blob/main/doc/source-maps.md)
- [Compiler/macro separation](https://github.com/squint-cljs/squint/blob/main/shadow-cljs.edn)
- [SCI macro environment](https://github.com/squint-cljs/squint/blob/main/src/squint/compiler/sci.cljs)

Source inspection used upstream commit `a98db4c`; execution used the pinned npm
release 0.15.212. Unreleased upstream behavior was not assumed to exist in that
release.
