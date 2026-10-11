---
name: contracts
description: Request structured answers with Due and interpret its result envelope, typed failures, and provider-specific limits.
---

# Use the result contract

`--output json` selects Due's final result envelope; it does not require the model
to answer in JSON. `--schema FILE` additionally asks the selected provider for an
answer matching its supported JSON Schema constraints. Use both for agent callers.

Save a provider-supported schema as `answer.schema.json`, for example:

```json
{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false}
```

```sh
printf '%s\n' 'Summarize this repository without changing files.' |
  due run --provider claude --cwd "$PWD" --stdin --timeout 5m \
    --schema answer.schema.json --output json
```

The same shared schema option works with Codex and with `--resume ID`.
Schema paths resolve from the caller's directory, independently of `--cwd`.
Due reads and snapshots the schema once, checking JSON syntax and an object or
boolean root. The provider enforces its schema subset; Due does not independently
validate constraints, convert incompatible schemas, repair answers, or retry.

- `schema_version` versions the envelope; the current value is `1`.
- `status` is `succeeded`, `failed`, `timed_out`, `cancelled`, or `planned`.
- `text` is the final answer, or available partial text on failure.
- `structured_output` retains native Claude data even without `--schema`; Codex
  final JSON is decoded only with `--schema`. It is independent of `text`; an empty successful
  Claude text answer falls back to the serialized structured value. JSON null
  can mean missing data or an explicit null answer in the serialized envelope.
- `session_id` is the native Claude session or Codex thread ID, not a Due session.
- `usage` holds nullable native counters; cache accounting differs by provider.
  Do not infer missing totals. `cost_usd` is available Claude cost, otherwise null.
- `duration_ms` includes Due input, setup, execution, and cleanup time.
- `error` contains `kind`, `message`, and nullable `provider_exit_code`.
  Trace-enabled runs also include `run_id` and a trace delivery summary.

Read typed errors before interpreting prose: `authentication`, `max_turns`,
`max_budget`, `configuration`, `session_not_found`, `unsupported`, `provider`,
`protocol`, `io`, `invalid_input`, `timeout`, `cancelled`, and `not_found`.
Unknown native failures remain `provider`. Missing Claude structured data or
invalid Codex JSON on an otherwise successful schema run becomes `protocol`.
An earlier native, timeout, or cancellation failure keeps its category.
Some buffered Claude cap errors expose only session and cost; metadata can be incomplete.

Exit codes are 0 for success/planning, 1 for execution failures, 2 for invalid
input, 124 for timeout, 127 for a missing executable, and 130 for cancellation.
After argument parsing, JSON mode emits one result even on runtime failure;
CLI syntax errors instead follow normal stderr/exit-2 behavior.
Dry runs have `status: planned` and an `invocation`, with no provider execution.

A valid schema or successful execution does not verify artifacts or side effects.
Check claimed changes and actions against the workspace or authorized external system.
Load `due skills show run` for execution or `due skills show observe` for progress.
