# Due Lua spike

`due-lua` is an opt-in Lua 5.4 runner for trusted workflows. Each invocation
owns one in-process `due-core` engine and can run several resident Claude
and Codex sessions. It shares the operation bridge, deadline, and cleanup
with [due-js](../due-js/README.md). Its bounded Lua VM and strict JSON codec
come from the earlier Lua/JavaScript comparison.

```sh
cargo build --locked -p due-lua
cargo run --locked -p due-lua -- path/to/workflow.lua '{"prompt":"Review the parser."}'
cargo test --locked -p due-lua
```

The executable uses its current directory and a five-minute workflow
deadline. Use `DUE_LUA_CLAUDE_BINARY` and `DUE_LUA_CODEX_BINARY` to select
provider executables. Scripts read `Due.args`, log with `Due.log(text)`, and
return one JSON value on stdout after cleanup. The library's `Options`
also accepts a directory and a deadline.

The scope operations match [due-js](../due-js/README.md) and have the same
argument order. In Lua, calls suspend the current task and return directly.
Use `scope.parallel(fn1, fn2, ...)` for concurrent work. It returns a JSON
array of branch results in argument order. An uncaught error in a branch
fails the workflow and the runner settles all workers.

Lua tables with string keys encode as JSON objects. Use `json.array` for
array results, `json.null` for a JSON null, and `ipairs` to traverse decoded
arrays. `scope.wait` accepts an ordinary sequence of run references.
Core failures raise a Lua error object with `kind` and `message` fields
that a task can inspect with `pcall`.

The [Lua workflow](tests/fixtures/two-provider.lua) and its
[JavaScript counterpart](../due-js/tests/fixtures/two-provider.js) run the
same fake Claude and Codex conversations concurrently. Both follow up
in the original sessions, page through session items, allow Claude's
approval request, and decline Codex's. The test compares both workflow
results and checks that each reads session items. Raw item counts can
vary with delivery timing.
