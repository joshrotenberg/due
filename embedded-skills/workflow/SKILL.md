---
name: workflow
description: Author and check an experimental bounded Rhai workflow with caller-declared Due profiles, schema gates, and private subprocess receipts.
---

Use this only with a binary built with Cargo feature `workflow-spike`. This is a
Unix execution spike, not a production release or an OS sandbox. For one task,
`due run` is cheaper and simpler. Existing native permissions still apply.

1. Read `due workflow --help`, `due workflow run --help`, and the `run` and
   `contracts` embedded skills. The caller authorizes profiles, permissions,
   workspaces and any model work. Do not add native permissions or delegation.
2. Write a manifest, schemas, input and script as below. Every profile needs an
   answer schema. Prepare workspaces yourself. Use read-only native profiles for
   parallel reviews; a cwd alone provides no isolation. Arbitrary native_args
   remain trusted caller configuration, including their native path semantics.
3. Run `due workflow check workflow.json --input task.json --output json`.
   Check compiles the entry and every declared module and validates definitions
   and input. It evaluates no script or module initializers and starts no child.
   It does not prove imports, dynamic branches, job IDs/budgets or final output.
4. Under the caller's existing authorization, use
   `due workflow run workflow.json --input task.json --artifacts new-receipt --output json`.
   The artifact directory must be new, and its parent must exist. SIGINT, SIGTERM
   or SIGHUP stop admissions and signal/wait for active ordinary Due children.
   Check retains normal OS signal handling; it has no child cleanup to await.
5. Inspect workflow.json and each step's step.json, result.json, stderr.txt,
   trace.jsonl and exit.json. Verify actual effects separately. Failed or
   cancelled jobs may already have edited files; there is no rollback or retry.

Minimal manifest (JSON v1; unknown fields fail):

```json
{
  "schema_version": 1,
  "entry": "main.rhai",
  "input_schema": "input.schema.json",
  "output_schema": "answer.schema.json",
  "max_runs": 3,
  "max_concurrency": 2,
  "profiles": {
    "reader": {
      "request": {
        "schema_version": 1,
        "provider": "codex",
        "cwd": ".",
        "schema": "answer.schema.json",
        "native_args": ["--sandbox", "read-only"]
      }
    }
  },
  "modules": {}
}
```

Request accepts existing Due v1 request fields except prompt (jobs supply it).
`cwd` defaults to the manifest directory; schema is required. A profile may add
`"binary":"./fake-agent"` beside request for a fake native provider executable.
Otherwise the provider is resolved from PATH before evaluation. Entry, schemas,
input, modules, cwd and binary resolve relative to the manifest directory and
are canonicalized. Sources, input and resolved profiles are snapshotted before
execution; child schema transport uses private snapshots. No process-wide chdir.
Executables and workspaces are referenced, not copied or made immutable.

For this example, input.schema.json is
`{"type":"object","required":["task"],"properties":{"task":{"type":"string"}},"additionalProperties":false}`;
answer.schema.json is
`{"type":"object","required":["approved"],"properties":{"approved":{"type":"boolean"}},"additionalProperties":false}`;
task.json is `{"task":"Review a synthetic proposal; do not edit or delegate."}`.

```rhai
import "patterns/v1" as p;
let reviews = p::review("review", "reader", input.task);
due::phase("synthesis");
p::answer(due::run(p::job("synthesis", "reader",
    "Reconcile findings: " + json::encode(reviews))))
```

Exact API:

- `due::run(#{id, profile, prompt})` returns one step outcome.
- `due::parallel(group, [jobs])` reserves the entire batch before any launch,
  runs at most max_concurrency children, and returns outcomes in input order.
  IDs are globally unique, 1..64 ASCII alphanumerics, `_` or `-`; group uses the
  same grammar. Jobs accept only id/profile/prompt. Prompt is 1..65536 bytes.
- `due::phase(text)` sets subsequent step metadata (at most 256 bytes).
  `due::log(text)` writes private workflow JSONL (at most 4096 bytes).
  Rhai print/debug are discarded; stdout is one workflow JSON envelope.
- `json::encode(value)` performs checked JSON conversion.
- Bundled `patterns/v1` exports `job(id,profile,prompt)`, `answer(step)`,
  `answers(steps)`, and `review(prefix,profile,task)` (two reviews, gated answers).
  `answer` throws unless execution and independent answer validation succeeded.

A step exposes id/profile/group/phase, ok, actual Due exit code/signal, error,
validation {valid,error}, answer_present, and artifact/child run references.
Only an ok step has answer. Native failures and invalid answers are values;
script/host errors stop the workflow, retaining partial receipts. A script may
handle an expected failed outcome explicitly. `p::answer` is the normal gate
before dependent jobs. `ok` says nothing about a task's approved/rejected verdict.
The child v1 envelope is preserved unchanged in result.json and step.json's
child_result. Host validation is beside it, not a rewrite of its
answer_schema_validation field.

The final expression is validated against output_schema. Stdout and workflow.json
use workflow_version:1, workflow_id, status, steps, output_present, and output on
success, or error on failure. Statuses checked and succeeded exit 0; failed exits 1 and cancelled exits 130
(CLI syntax errors exit 2). Failure before directory creation has
no artifacts. Missing final workflow.json indicates an incomplete/crashed run.

Local composition: declare `"modules":{"local/review":"review.rhai"}` and
import `"local/review" as review`. Only function definitions are extracted;
all top-level module statements, imports, constants and effects are discarded.
Import dependencies inside functions. Local functions can call the same due/json
primitives as bundled functions. Module names must be local/NAME; patterns/v1,
due and json are reserved. Host namespace names cannot be rebound. Input is a
constant. No eval, filesystem resolver, shell, filesystem or network API is
exposed. Blocking sleep, timestamps and Rhai's permissive parse_json are absent.
Backtick interpolation is unsupported; use concatenation.

Schemas use a restricted Draft 2020-12 dialect. Supported keywords: $schema
(exact https://json-schema.org/draft/2020-12/schema), type, enum, const, required,
properties, additionalProperties, items, allOf, anyOf, oneOf, not, minimum,
maximum, exclusiveMinimum, exclusiveMaximum, minLength, maxLength, minItems,
maxItems, minProperties, maxProperties, uniqueItems, title, description. Boolean
schemas work. Every other keyword, including $ref, $id, format and pattern, is
rejected; network/file schema resolution is disabled. JSON null maps to Rhai (),
missing map properties error. Integers must fit i64, floats must be finite, and
integral floats outside the exact +/- (2^53-1) range are rejected. Character,
blob, function-pointer and other non-JSON values are rejected.

Fixed limits: 1..64 runs, 1..8 concurrency (no greater than max_runs), 1..32
profiles, <=32 local modules, 1 MiB per source/JSON value/child stdout/stderr/trace,
32 JSON/schema nesting, 100000 VM operations, 32 call depth, 4096 array items,
256 map entries/variables, 128 functions and 64 module imports. Workflow logging
has a 2048-record/1 MiB budget. Batches join in concurrency-sized chunks. Output
overflow stops admissions, signals children, drains pipes, and retains truncated
bytes plus the error; trace overflow is recorded by the child's trace.incomplete.
No provider deadline is invented; caller request.timeout remains available.
Cancellation awaits wrapper cleanup; there is no forced-kill deadline for an
unresponsive Due child or escaped descendant accounting. Native processes can
consume resources beyond these host limits. Artifacts have private Unix modes,
not protection against a provider with the same user permissions. Do not put
untrusted writable workloads around the receipt directory. This spike does not
verify workspace overlap, effects, authentication, native flag support or costs.
No retries, resume, scheduler, server, tools, workspace creation or registry.
