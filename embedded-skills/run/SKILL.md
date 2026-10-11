---
name: run
description: Run one bounded Claude Code or Codex task through Due and independently verify its artifacts and effects.
---

# Run a task

Due wraps one noninteractive `claude -p` or `codex exec` invocation.
It is one foreground CLI run; the caller and OS own concurrency and scheduling.
Use the provider the user selected, with its installed CLI, authentication,
settings, and native permissions. Due does not grant additional authorization
or make the providers' permission policies equivalent.

Define the concrete deliverable, working directory, allowed changes, and stopping
condition in the prompt. Carry existing user authorization into the task without
expanding it. Choose a timeout appropriate to that bounded task; Due has no default.
Inspect existing workspace changes when they matter to later verification.

Send the prompt through stdin and request the result envelope for agent callers:

```sh
due run --provider codex --cwd "$PWD" --stdin --output json --timeout 5m <<'PROMPT'
Inspect this repository without changing files. Identify the failing test's
likely cause and report the relevant files and evidence. Stop after diagnosis.
PROMPT
```

Replace the example scope with the actual assignment. `--provider claude` uses
the same shared options. Omit `--model` and `--effort` to retain native defaults;
model names and supported effort levels remain provider-specific.
With execution flags, exactly one of `--stdin` and `--prompt TEXT` is required. Both reach the child
through stdin, but `--prompt` exposes the prompt in Due's own argv.

A request file can hold a complete task or a partial profile. Only
`schema_version: 1` is required in the file; provider and prompt must be supplied
after merging. Explicit CLI execution flags replace file values; absent flags
inherit them. For example, save this as `profile.json`:

```json
{"schema_version":1,"provider":"codex","timeout":"5m","ephemeral":true}
```

```sh
due run --request profile.json --stdin --dry-run --output json < task.txt > plan.json
due run --request profile.json --stdin --output json \
  --trace --trace-file run.jsonl < task.txt > result.json
run_status=$?
due trace summarize run.jsonl --output json > trace-summary.json
```

Use a fresh trace path. Preserve `run_status`; the summary command has its own
exit status. Add `--schema answer.schema.json` when the assignment needs a typed
answer. `due schema request|result|trace` exports each installed contract (choose
one name). Load `due skills show contracts` for input and result rules.

`--no-ephemeral` overrides profile true. Native arguments after `--` replace the
whole profile list; `--clear-native-args` clears it. Paths for cwd and schema use
the caller's directory. `--request -` reads a whole JSON document from stdin and
cannot share stdin with `--stdin`; a CLI `--prompt` can override that document.
Changing provider does not clear other inherited options.

Add `--trace` for readable progress, or load `due skills show observe` for JSONL
monitoring. Load `due skills show contracts` when the caller needs structured
answers or must interpret result fields. `due skills` lists the available skills;
`due skills list --output json` provides the catalog for automated selection.
`due skills show NAME --output json` also returns a machine-readable response.

Use `--dry-run` to validate the merged task, native conflicts, cwd, and schema
JSON without launching a provider. It works without an installed provider and
does not verify authentication, model availability, or provider schema support.
It omits prompt text but shows native option values and the inline Claude schema;
requested tracing and temporary schema snapshots may still write files.
Pass native options after `--`, for example `-- --max-turns 5` for Claude.
Use shared flags for cwd, prompt transport, output, and session selection.
Passthrough accepts options, not another command or positional prompt. For an
unknown option whose value begins with `-`, use `--option=value`.
Native permissions remain in force; do not add bypass flags as a default.

Use `--resume ID` only for the intended provider's session and authorized follow-up.
It requires a new prompt. `--ephemeral` disables persistence and conflicts with
resume; an ID returned by an ephemeral run may not be resumable.

After the run, inspect its exit status and result. For structured answers, require
`structured_output_present` and independently validate the value when correctness
is a gate; `answer_schema_validation` does not claim independent validation.
Then inspect the actual
deliverables: changed files, diffs, generated artifacts, relevant checks, or the
external system the task was authorized to change. A provider's narrative and
activity records are reported evidence; label effects as verified only after
independent observation. Preserve unrelated workspace changes.

Do not retry automatically after failure, timeout, or cancellation. Report the
partial outcome and inspect possible effects before any authorized follow-up.
Due does not roll back changes or collect a verified side-effect inventory.
