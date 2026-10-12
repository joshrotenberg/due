---
name: workflow-fennel
description: Author an experimental Fennel function over the optional Lua workflow runtime, with bounded offline compilation and manifest-only imports.
---

This guide exists only in `workflow-fennel-spike` builds. Read the shared
`workflow` guide for Rust-owned jobs, profiles, required schemas, JSON rules and
native cleanup. No new permission, provider, scheduler or macro API is added.

Use manifest `schema_version:2`, entry `main.fnl` (or `.lua`), and the same other
manifest fields as v1. Entry loading runs with every Due effect disabled, must
return a function, then Rust calls that function once with input asynchronously.
V1 Lua chunks still execute directly; v1 Rhai remains separate. Fennel entries
require v2. Declared Lua and Fennel modules can import each other, returning
plain function tables with at most 128 exports; cache/cycle rules are shared.

```fennel
(local p (require :patterns/v1))
(fn [input]
  (local h (due.start (p.job :review :reader input.task)))
  (p.answer (due.wait h)))
```

Check with `due workflow check workflow.json --input input.json`. It starts zero
agents and executes no workflow initializer. It DOES execute the pinned compiler
and built-in macros in an isolated helper process, bounded and joined by Due.
Run only within existing authorization using
`due workflow run workflow.json --input input.json --artifacts new-receipt`.
Inspect receipts and verify native effects independently.

Ordinary fn/lambda, closures, destructuring/rest, let/var/set, if/when/match/case,
loops, comprehensions, threading, partial, doto and multi-return are supported.
No user macros/macros/import-macros/require-macros, eval-compiler, include, raw lua,
quote/unquote, with-open, assert-repl, macrodebug or global forms. These are
rejected by compiler dispatch, not text matching. Modules export functions.
`require` resolves only manifest declarations or patterns/v1; there are no
ambient FENNEL_PATH/LUA_PATH/package/native loaders. Removed Lua library methods
remain unavailable even through `_G`. There is no runtime Fennel dependency.

Use `json.null` for explicit null, `(json.array)` for empty arrays and
`(json.object)` or `{}` for empty objects. Fennel `[]` is an untagged empty Lua
table and becomes an object. JSON integers remain exact i64; nonfinite/lossy
floats, mixed/sparse/cyclic tables and handles are rejected. Sequence rest uses a
getmetatable compatibility shim that ALWAYS returns nil; internal JSON/handle
metatables cannot be obtained. Runtime setmetatable stays absent.

Compiler limits: 128 KiB per Fennel file, 512 KiB total Lua/Fennel bundle source,
32 MiB Lua allocation, 20 million instructions shared across compilation, 4096
compiler events per file, AST depth 64 / 32768 table nodes, 512 KiB generated Lua
per file / 1 MiB total, bounded source maps, and a 5 second helper deadline.
Cancellation kills/joins the compiler, including C-library work; provider
request deadlines are unchanged. Workflow limits remain 16 MiB / 100000
instructions with uncatchable outer-future termination. This is not a proof of
hostile-code or native-process isolation.

Run snapshots include original Fennel, generated Lua, compiler version/hash,
fixed options, integration driver and detailed line maps. Diagnostics retain
raw generated locations and host-only last-workflow locations across async
waits. Some synthetic/macro-generated lines have no original mapping; raw Lua
locations remain authoritative there. Public `correlate` is deliberately off.

Fennel is an authoring convenience over the Lua candidate, not a cure for Lua's
JSON, embedding, C-loop or lifecycle tradeoffs. Rhai and Lua feature combinations
exist for comparative tests, not a decision to ship two runtimes. Default builds,
ordinary due run, wrapper pins and plugin/install policy are unchanged.
