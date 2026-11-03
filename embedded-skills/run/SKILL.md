---
name: run
description: Scope, execute and independently verify one bounded Claude Code or Codex assignment, with optional native-session resume.
---

# Run one assignment

`due run` executes one noninteractive Claude Code or Codex turn. Give the worker
its own self-contained assignment: workspace, context, deliverables, constraints,
permitted changes and a stopping condition. It does not inherit the calling
agent's conversation, model or per-turn authorization. Honor the user's provider
choice and carry existing scope into the prompt without widening it.

For a resident worker with live items, permission replies or several supervised
turns, discover `script`/`rpc` with `due -h` and read `supervise`/`rpc` when listed.
The caller owns scheduling, concurrency, verification and authorized follow-ups.
To decide whether a task suits Due at all, or to combine runs into review,
fan-out or extraction patterns, read `due skills show use-cases`.

## Run and inspect

Use stdin for longer prompts and JSON for programmatic results:

```sh
due run --provider claude --cwd "$PWD" --stdin --output json <<'PROMPT'
Read README.md and the parser implementation. Explain the main edge cases and
relevant tests. Do not change files or delegate. Stop after diagnosis.
PROMPT
```

`--provider codex` uses the same shared options. Omit model/effort to use native
defaults. `-p TEXT` also reaches the provider through stdin, but exposes the text
in Due's own argv. `--stdin` and `--prompt` cannot be combined.

After completion, inspect the exit status, `result.status`, `result.error`, and
the actual artifacts or diff. A successful worker report does not prove changes
were accepted, a schema was independently validated, or an external effect
occurred. Read `due skills show contracts` for exact fields and error categories.

## Reuse configuration and preview

Save this as `worker.json` in a caller-owned location:

```json
{"schema_version":1,"provider":"claude","ephemeral":false}
```

Supply the task separately, preserving its result and exit code:

```sh
due run --request worker.json --stdin --dry-run --output json < task.txt
run_status=0
due run --request worker.json --stdin --output json < task.txt > result.json || run_status=$?
printf '%s\n' "$run_status" > result.exit
```

A request may hold a complete task or partial defaults. Explicit flags replace
file values; omission inherits them. Native options after `--` replace the whole
profile list, including permission settings; `--clear-native-args` empties it.
`--no-ephemeral` overrides profile true. `cwd` and answer-schema paths resolve
from the caller directory, not the profile directory. Read `due skills show profiles` for fields, path rules and complete Claude/Codex examples.

`--request -` reads a JSON request from stdin and cannot share it with `--stdin`:

```sh
due run --request - -p 'Summarize without changing files.' --dry-run --output json <<'JSON'
{"schema_version":1,"provider":"codex"}
JSON
```

`--dry-run` validates the merged task, native conflicts, cwd and schema JSON,
then prepares a plan without starting a provider. It does not verify login,
model availability, native option support, effective permissions or the native
schema subset. Plans omit prompts but can include native option values and
schema content. Trace preparation and schema snapshots can still create files.

## Observe and choose a deadline

```sh
run_status=0
due run --request worker.json --stdin --output json \
  --trace --trace-file new-run.jsonl < task.txt > result.json || run_status=$?
due trace summarize new-run.jsonl --output json > summary.json
# Keep run_status: summarize has its own exit status.
```

Stdout is one final answer/result. Stderr carries diagnostics or live progress;
`--trace=json` selects JSONL rather than readable progress. `--trace-file` alone
records JSONL without live stderr progress. Trace files must be new. Detailed
activity text is opt-in with `--trace-detail`; it can contain task content.
Read `due skills show observe` for delivery loss, summaries and cancellation.

There is no default timeout. A monitored task can omit it; choose a deliberate,
task-sized `--timeout`, such as `45m`, for a time budget or unmonitored run.
A profile timeout still applies when the flag is omitted. The deadline is
absolute from provider start: activity does not reset it and it cannot be
extended mid-run. Heartbeats mean the process is waiting, not that it is making
progress. Judge artifacts/events before deciding to cancel.

## Structured answers

```sh
due run --request worker.json --stdin --output json \
  --schema answer.schema.json < task.txt > result.json
```

`--output json` selects the Due result envelope; `--schema FILE` supplies the
caller's desired model-answer schema. Use a schema supported by the chosen
provider. Due parses/snapshots that file but does not independently validate
constraints, repair answers or retry. Check `structured_output_present`, and
validate `structured_output` yourself when correctness gates later work.
`due schema result` exports Due's envelope, not an answer schema. See `contracts`.

## Continue saved native context

After independently checking the previous result, pass its native `session_id`
as `ID` in an authorized follow-up:

```sh
due run --provider claude --resume ID -p 'Explain the main finding.' --output json
```

Each invocation needs a new prompt. Save the ID programmatically from the result
rather than guessing or using an unrelated session. Resume needs the same
provider, its persisted history and the intended workspace. It is not a proven
way to relocate native history to another workspace. Ephemeral runs may return
IDs, but cannot be resumed; later disabling ephemeral mode cannot recover them.
Due has no implicit last-session selection or private session database.

## Native permissions and options

Provider authentication, settings and permissions remain in force. Due does not
make Claude and Codex permission policies equivalent, grant new authorization or
disable native subagents by default. State whether delegation is allowed.
Profiles can carry task-scoped native controls; see `profiles` for examples and
the narrower resident-session allowlist.

Use shared flags for prompt, output, cwd and session selection. Additional native
options follow `--`, without an implicit shell:

```sh
due run --provider claude -p 'Review only.' --dry-run --output json -- --max-turns 3
```

Passthrough accepts options, not a second command or positional prompt. For an
unknown native option whose value starts with `-`, use `--option=value`.
Due reserves transport/session/help options and rejects native equivalents that
conflict with explicitly selected shared model, effort or schema options.
Check the installed provider's help before relying on a native option.

Native permission rules are not an isolation boundary for repository code run
by an approved test command. Requested sandbox settings and a successful dry
run do not establish effective permissions. Preserve managed restrictions and
report denied checks without automatically expanding grants or adding bypass
flags. Keep caller-owned profiles, schemas and acceptance evidence outside
worker-writable roots when those files define policy.

## Failure and cancellation

Ctrl-C, SIGTERM and SIGHUP request cancellation and cleanup. An execution timeout
exits 124; handled cancellation exits 130. Cleanup can extend past a deadline.
SIGKILL cannot be handled and may leave provider/tool processes. Cancellation
and failure do not roll back edits; preserve results and inspect partial effects
before an authorized follow-up. Due does not automatically retry.

For launch/PATH, authentication, sandbox, resume and schema problems, read
`due skills show overview`. A sandbox-only login failure is not proof of expired
credentials; check the provider in the normal host context. Interactive login
belongs to the user. Never copy credentials or change global policy to fix a run.

Use `due run --help` for every option and its examples; `profiles`, `contracts`
and `observe` hold the corresponding detailed contracts.
