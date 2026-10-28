# Squint authoring proof

Pinned Squint 0.15.212 compiles these ClojureScript-like sources into ES modules
for the unchanged QuickJS spike. Node is used only to compile and package them;
Due runs the generated JavaScript using the same scope-bound host API.

From this directory, with Node 22 or newer:

```sh
npm ci --ignore-scripts --no-audit --no-fund
npm run build
```

The build captures Squint's unmodified `src/squint/core.js`, compiles the two
`.cljs` modules, and maps their core import to the manifest's `local/squint-core`
name. It also copies the adjacent JavaScript example's input, schemas, and
profiles. Generated output and dependencies are ignored by Git. Rebuilding
replaces the generated files, including input and manifest edits in `dist/`.

Copy `dist/` into protected caller-owned storage before configuring a run.
Follow the [JavaScript example's workspace and permission setup](../quickjs/README.md):
provide two explicitly authorized isolated workspaces, update the task, and
review native grants. Use the spike executable explicitly:

```sh
cd /path/to/configured-bundle
/absolute/path/to/spike/due workflow check workflow.json --input input.json
/absolute/path/to/spike/due workflow run workflow.json --input input.json --artifacts /new/private/path
```

The entry calls a reusable Squint pipeline twice concurrently, replaces one
review stage with an ordinary JavaScript function, uses collection helpers to
prepare findings, and synthesizes the results. That JavaScript review function
calls the Squint stage function back through an ES module import. The original
JavaScript example's `main.js` also works with the compiled Squint pipeline.

Return plain maps, vectors, strings, numbers, booleans, and nil. Materialize lazy
sequences with `vec` or `mapv` before crossing Due's JSON boundary, including
nested values. Pass `due` explicitly; dynamic `binding` is shared mutable state,
not async-local context. Use `due.log` instead of relying on a console.

Source maps are emitted for inspection, but this Due spike still reports
generated `.js` stack locations. It does not load `.cljs`, embed the compiler,
load custom macros, install packages, or provide Node APIs at workflow runtime.
See the [compatibility report](../../../docs/workflow-squint-spike.md) for tested
behavior and remaining integration questions.
