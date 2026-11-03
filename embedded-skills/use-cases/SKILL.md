---
name: use-cases
description: Decide whether a task suits Due and match it to a pattern (bounded delegation, multi-turn specialist, review and revise, independent assignments, structured extraction, embedding in a program), with one provider or two.
---

# Use cases and patterns

Due gives a caller a separate Claude Code or Codex session with its own context,
working on an assignment the caller wrote and returning a result the caller
checks. That is useful with only one provider installed: a fresh session of the
same provider has none of the caller's conversation, so it can work on a narrow
brief, carry a large context the caller does not want to hold, or review work it
did not write. Using the other provider as well is an option for a second
opinion, not a requirement.

The patterns below apply to any kind of work the provider can do with the tools
and permissions it already has. Coding, document drafting, research and data
handling are examples, not separate modes. This guide chooses a pattern and
points to the guide that holds the mechanics; it does not repeat them.

## When something else is simpler

- **Do it directly** when the task is small, depends on what you already know
  from this conversation, or would take longer to brief than to do.
- **Use the provider's interactive CLI** (`claude` or `codex`) when a person
  wants to steer turn by turn, answer prompts as they come, or explore without a
  defined deliverable or stopping point.
- **Use Due** when the work can be written as a self-contained assignment with a
  deliverable and a stopping condition, and the caller (agent, script or person)
  will judge the result from artifacts rather than from the worker's report.

## Patterns

| Pattern | Typical shape | Start with |
| --- | --- | --- |
| Bounded delegation | One assignment, one answer or change | `due run`; guide `run` |
| Context-heavy specialist | Several turns building on what the worker already read | `due run --resume ID`, or a resident session; guides `run`, `supervise`, `rpc` |
| Review, verify, revise | A fresh worker critiques work; the caller checks; the author revises | `due run` for review; `--resume` or `due script` for revision |
| Independent assignments | Several unrelated tasks at once | Separate `due run` processes the caller starts and collects |
| Structured extraction or transformation | Output another step consumes | `due run --schema FILE --output json`; guide `contracts` |
| Embedding in a caller | A program or pipeline uses Due as a step | `--output json` and exit codes, profiles, or `due rpc`; guides `contracts`, `profiles`, `rpc` |

`script` and `rpc` and their guides exist only when `due -h` and `due skills`
list them. Reduced builds support one-shot assignments and saved-context
follow-ups with `due run` and `--resume`; live resident supervision requires a
build that includes `script` or `rpc`.

### Bounded delegation

Write the workspace, inputs, deliverable, allowed changes and stopping point
into one prompt, run it, then inspect the exit status, `status` and the real
artifacts. Examples: fix one failing test; summarize a folder of meeting notes
into a decision log; draft a changelog from the commits on a branch.

### Context-heavy specialist

Have one worker read a large body of material once, then ask it follow-ups.
`due run --resume ID` continues the saved native session from a previous
result in a separate process; it needs the same provider, persisted history
(not ephemeral) and the intended workspace. Use a resident session through
`due script` or `due rpc` when you also need live items, permission replies or
interruption between turns. Examples: a worker that has read a codebase's
storage layer answering a series of design questions; a worker that has read a
contract set answering clause-by-clause queries.

Resuming requires the explicit native session ID from a result. Due keeps no
session registry and has no way to attach to another process's live session;
named sessions and live attachment are experimental and not part of this build.

### Review, verify, revise

A fresh session reviews a diff, document or analysis it did not produce. The
same provider is a valid reviewer because the reviewer starts with a clean
context; a different provider adds a second model's view when it is available
and wanted. The caller runs the objective checks (tests, linters, a schema
validator, source comparison) itself and treats the reviewer's verdict as
input, not acceptance. If revision is needed, resume the author with the
findings or run the loop in `due script` (`due skills show supervise`).
Examples: a second Codex session reviews a Codex patch against the issue text;
a Claude session fact-checks a research summary against the cited files.

### Independent assignments

Start one `due run` per assignment and collect each result and exit status
separately. Due has no scheduler, worker pool or queue: the caller and the OS
own concurrency, rate limits and retries. Due does not isolate concurrent
workers from each other. Give writing workers separate workspaces (for example
separate checkouts or output directories) and keep shared inputs read-only.
`due script` and `due rpc` hold one session per process, so concurrent resident
workers are separate processes too.

```sh
mkdir -p results
for task in tasks/*.txt; do
  name=$(basename "$task" .txt)
  ( status=0
    due run --request worker.json --stdin --output json \
      < "$task" > "results/$name.json" || status=$?
    printf '%s\n' "$status" > "results/$name.exit" ) &
done
wait
```

Examples: triage ten unrelated issues read-only; convert several independent
data files, each into its own output path.

### Structured extraction or transformation

Ask for an answer that matches a JSON Schema the caller owns, then validate
`structured_output` yourself before anything depends on it. Due passes the
schema to the provider and does not validate, repair or retry. Examples:
extract fields from invoices into records; classify log excerpts; turn
free-form findings into a typed review verdict. Read `due skills show contracts`.

### Embedding in a caller

A shell script, CI job or program can call `due run --output json` and branch
on the exit code and `status`, keeping each result even when a run fails.
Versioned JSON profiles carry provider and native settings between calls
(`due skills show profiles`). A program that needs to hold a session and react
to streamed items can speak the stdio JSON-RPC protocol directly
(`due skills show rpc`). Observe progress with traces (`due skills show observe`).

## What stays with the caller

Due adds no authorization, sandbox, scheduler, central session broker or retry policy. Provider
authentication and permissions apply as configured. The caller writes the
scope, chooses the provider, decides concurrency, verifies effects and decides
whether to follow up. Preserve results, stderr and partial edits from failed or
cancelled runs before acting on them.
