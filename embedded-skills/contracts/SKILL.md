---
name: contracts
description: Request structured answers with Due and interpret its result envelope, typed failures, and provider-specific limits.
---

# Use the result contract

`due schema request`, `due schema result`, and `due schema trace` export the
installed contracts as JSON Schema without a provider or network.
`due schema trace-summary` describes offline summaries and their error envelope.

`due run --request FILE` accepts a complete task or partial profile;
`--request -` reads the document from stdin. Only `schema_version: 1` must be in
the file. Explicit CLI execution flags replace file values; omitted flags inherit.
Provider (`claude` or `codex`) and a non-whitespace prompt must exist after merging,
or use `--stdin` for the prompt. Optional file fields are `model`, `effort`,
`cwd`, `timeout`, `schema`, `resume`, `ephemeral`, and `native_args`.

```sh
due run --request - --prompt 'Summarize without changing files.' --output json <<'JSON'
{"schema_version":1,"provider":"claude"}
JSON
```

Malformed JSON, unknown/duplicate keys, unsupported versions, and bad JSON types
fail even if CLI flags would replace them. Semantic checks follow merging: an
invalid duration string can be replaced with `--timeout 5m`. Provider/prompt may
be omitted but not null. Nullable optional fields mean absent; model/resume must
be nonempty. Effort is low/medium/high; timeout is a positive duration. Due has
no default timeout; omitting `--timeout` inherits a file value, so remove the
field or set it null to run without that deadline.
`ephemeral` is a boolean defaulting false; `native_args` is a string array defaulting
empty. Neither accepts null. `--no-ephemeral` overrides profile true; final resume
conflicts with ephemeral true. Native arguments after `--` replace the entire list;
`--clear-native-args` clears it, while a bare `--` inherits it.

`--prompt` and `--stdin` each replace the file prompt. `--request - --stdin` is
invalid because both would consume stdin. Request cwd/schema paths use the
caller's cwd, not the file directory. Native argument paths use provider rules.
Output, binary, dry-run, and tracing controls remain CLI-only. The request schema
checks shape; `--dry-run` also checks the merged task, native conflicts, cwd, and
schema JSON before preparing a plan. It does not validate authentication/model
support or the provider's schema subset. This is one foreground run.

`--output json` selects Due's final result envelope; it does not require the model
to answer in JSON. `--schema FILE` additionally asks the selected provider for an
answer matching its supported JSON Schema constraints. Use both for agent callers.

Save a provider-supported schema as `answer.schema.json`, for example:

```json
{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false}
```

```sh
printf '%s\n' 'Summarize this repository without changing files.' |
  due run --provider claude --cwd "$PWD" --stdin \
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
  Claude text answer falls back to the serialized structured value.
- `structured_output_present` is false for missing data and true for a supplied
  answer, including explicit JSON null. Never use value truthiness to check presence.
- `answer_schema_validation` is `not_requested` without the common schema option
  or `not_performed` with it, even on planned/failed runs. Intake failures use
  `not_resolved` when schema intent is unknown; an explicit CLI schema is known.
  The field describes Due's
  independent validation, not native enforcement. Validate the answer separately
  when validity gates further work; neither presence nor execution success proves it.
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
Request intake errors use `provider: null`, `invalid_input`, and exit 2. Runtime
errors after resolution identify the selected provider in the same envelope.
Intake precedes tracing: errors may print plain stderr even with `--trace=json`,
and no trace file is created when intake fails.
Dry runs have `status: planned` and an `invocation`, with no provider execution.

A valid schema or successful execution does not verify artifacts or side effects.
Check claimed changes and actions against the workspace or authorized external system.
Load `due skills show run` for execution or `due skills show observe` for progress.
