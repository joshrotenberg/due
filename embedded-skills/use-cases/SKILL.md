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

### Choose a session lifecycle

| Surface | Fresh | Live reuse | Saved resume | Saved fork |
| --- | --- | --- | --- | --- |
| `due run` | Run without `--resume` | Unavailable | `--resume NATIVE_ID` | Unavailable |
| `due rpc` | `session.open` without `resume` | Start another turn on the open session | `session.open` with `resume: NATIVE_ID` | Unavailable |
| `due script` | `scope.open(profile)` without `resume` | Call `worker.turn` again on the open worker | Open with `resume: NATIVE_ID` | Unavailable |
| Rust `due_core::Engine` (resident feature) | `spawn` | `follow_up(SessionRef, ...)` | `follow_up(SavedSession, ...)` | `fork(SavedSession, ...)` |

Fresh starts a new conversation. Live reuse keeps the open native process and
its context. Saved resume opens another process on the same persisted native
history. Saved fork starts a new native history from an existing one, with a
new Due handle and native ID.

For saved operations, use the same provider and intended workspace, and keep the
source idle with persisted history. An ephemeral worker can take live follow-ups
where supported. It has no saved history to resume or fork. Use the native
session or thread ID for CLI, RPC and script resume. A Due handle is scoped to
the owner that created it.

The Rust fork adapter supports Claude and Codex. The installed provider must
accept the fork for the requested history. Due reports native rejection as a
failed run with the provider diagnostic and requires a distinct native ID before
offering a saved descriptor for the branch. A fork carries conversation context.
Workspace files are shared when both workers use the same workspace.

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
For issue work, start one author and one fresh reviewer. Give the reviewer
the issue, diff and relevant repository evidence. Keep the two session IDs
separate. If the author revises the change, continue the author with the findings
and continue the reviewer with the new diff. Set a stopping point for revisions,
then close resident sessions or stop resuming saved sessions when the issue is
done. Either role can use either provider. A fork carries the source's history,
which is useful for exploring a different approach and changes what the reviewer
has seen.

The [runnable author/reviewer example](https://github.com/joshrotenberg/due/blob/main/examples/core-compare/tests/author_reviewer.rs)
uses fake Claude and Codex workers to demonstrate separate live follow-ups and
cleanup. Run it from a Due source checkout on macOS or Linux:

```sh
cargo test --locked -p due-core-compare --test author_reviewer -- --nocapture
```

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
