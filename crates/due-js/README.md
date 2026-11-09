# Due JavaScript spike

`due-js` is an optional embedded QuickJS runner. One workflow owns one
`due-core` engine and can use several resident Claude and Codex sessions.
The runner shares the QuickJS evaluator, Promise tracking, and strict JSON
encoder with the existing `due script` runtime. Its scope methods call the
engine directly.

```sh
cargo build --locked -p due-js
cargo run --locked -p due-js -- path/to/workflow.js '{"prompt":"Review the parser."}'
cargo test --locked -p due-js
```

The executable starts providers in its current directory. The host can set
`DUE_JS_CLAUDE_BINARY` and `DUE_JS_CODEX_BINARY` to select executables.
The library also accepts a directory and workflow deadline through
`due_js::Options`. Call `due_js::run` on a synchronous owner thread.
The executable uses a five-minute workflow deadline.

A trusted script calls `Due.run(async scope => { ... })` once. All scope
operations return Promises, and independent operations can run in parallel
using `Promise.all`. The body may return a JSON value, which becomes stdout
after worker cleanup. Use `Due.args` to read the argument document and
`Due.log(text)` to write a message to stderr.

| Operation | Behavior |
| --- | --- |
| `spawn(provider, prompt, timeoutMs?)` | Starts a worker and returns a run snapshot with live session and run references. |
| `followUp(session, prompt, timeoutMs?)` | Starts the next turn in the same live session. |
| `inspect(run)` | Returns a snapshot with pending interactions or a terminal result. |
| `wait(runs, timeoutMs = 1000)` | Observes completion of all supplied runs. Zero polls, and the maximum budget is ten seconds. |
| `interrupt(run)` | Interrupts the exact run and returns its settled snapshot. |
| `respond(interaction, response)` | Submits a provider-native object to an exact pending interaction. |
| `readItems(session, after = 0, limit = 32)` | Reads a page of full retained items, with an exclusive cursor for the next read. |

Use a token from `inspect().interactions[].id` or a session item's
`interaction` field. The script decides how to handle each request.
Core operation failures reject with an error that has a `kind`. An admitted
run's failure is data in its terminal result. Wait expiry leaves work active.
The runner settles workers after normal return, workflow errors, or a
deadline. A workflow with unresolved core operations cannot publish a result.

The [two-provider fixture](tests/fixtures/two-provider.js) shows the complete
workflow with fake executables. It runs a fresh turn with both providers,
uses each session for a follow-up, reads approval tokens through item pages,
and sends explicit decisions. Its synthetic approval policy is specific to
the fixture.
