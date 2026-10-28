---
name: workflow-quickjs
description: Author an experimental JavaScript workflow using scoped Due capabilities and strict JSON.
---

# JavaScript workflow spike

Use a manifest v2 with a `.js` ES module entry exporting a default function:

```js
export default async function(due, input) {
  const outcomes = await due.concurrent("reviews", [
    async child => child.run({id:"a",profile:"claude",prompt:input.prompt}),
    async child => child.run({id:"b",profile:"codex",prompt:input.prompt})
  ]);
  return outcomes;
}
```

Declare profiles, input/output schemas, native capacity and run bounds in the
manifest as described by the workflow CLI. Profiles own permissions, executable,
workspace and per-run answer schemas. Jobs accept only id/profile/prompt.
Use `workflow check manifest.json --input input.json` before `workflow run`.
Check compiles all modules without initializing them or starting jobs.

The first argument is a scope-bound capability. Pass it explicitly through reusable
functions. concurrent callbacks receive their own capability; use that argument
for child work and return JSON outcomes, never run handles. Await all work.
start admits synchronously; wait/wait_any/cancel return promises. run combines
start and wait; parallel admits a batch atomically. phase/log are synchronous.
Native failures are outcomes with ok=false. Host contract failures terminate the
whole invocation even if caught. Cancellation retains effects and joins children.

Import only manifest-declared exact names such as local/pipeline or patterns/v1.
No Node, browser, network package loader, filesystem or subprocess API is exposed.
JSON uses ordinary objects, arrays and null. Undefined, functions, BigInt, unsafe
integers, proxies, getters, custom prototypes, cycles and sparse arrays are rejected.
Keep return values within the shared depth/size/count limits. Inspect actual
receipts and effects independently. See docs/workflow-quickjs-spike.md in source
for comparison limits and acceptance evidence. This is an optional frozen-branch
experiment, not the installed Due command set.
