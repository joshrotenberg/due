---
name: work-loop
description: Carry out a larger order (a backlog of issues, one hard change with independent review, recurring upkeep) by splitting it into pieces, running each piece through Due workers, and continuing without the human except for decisions that cannot be undone or are out of scope.
---

# Run an order through Due workers

Use this when a person gives you an order larger than one assignment and
expects you to keep working: "land these issues", "make this change and have
it reviewed", "keep these projects healthy". You are the caller. You hold the
order, split it into pieces, run each piece through a Due worker, check every
result yourself, and keep one board and one inbox. For a single assignment,
use the `delegate` skill instead.

Due runs the workers. It does not schedule, retry, or decide. Those stay with
you, and this guide is the procedure. Read `due skills show use-cases` and
`due skills show run` once if you have not used Due in this session.

## What goes to the person, and what does not

Ask the person only for these:

- Merging, releasing, publishing, or deploying.
- Deleting remote branches, issues, data, or anything not created by this order.
- Spending money or using an account the order did not name.
- Work outside the order's repositories or stated scope.
- A choice between directions that would be expensive to reverse.

Decide everything else. If a choice can be undone (a name, a file layout, an
approach among several that pass the same check), make it, record it as an
assumption on the board, and continue. A question to the person parks one
piece. It never stops the others.

At the start, read the order once for gaps that would block every piece
(which repositories, what counts as done, anything the person added to the
ask-first list). Put those in a single message, then begin with the pieces
that do not depend on the answers.

Who commits and pushes is the operator's choice. The order can say that
workers commit, or commit and push, or that the caller does both after
checking. Record the choice in `order.md`. If the order does not say, the
caller commits and pushes, because that needs no extra worker permissions and
every change is checked before it leaves the machine.

## Keep the state on disk

Create a work directory outside every checkout. This guide calls it `$WORK`.
A conversation can be compacted, replaced, or moved to the other provider, and
the next caller continues from these files.

| File | Holds |
| --- | --- |
| `$WORK/order.md` | The order in the person's words, the scope, the ask-first list |
| `$WORK/board.md` | One row per piece (see below), updated at every state change |
| `$WORK/inbox.md` | Open questions and approvals for the person, with the piece each one blocks |
| `$WORK/work-style.md` | Working instructions every worker receives, from the person or the repository |
| `$WORK/profiles/` | One JSON profile per provider and permission set |
| `$WORK/result.schema.json` | The worker result schema below |
| `$WORK/run-piece.sh` | The launch script below |
| `$WORK/pieces/ID/` | `assignment.md`, and per attempt `result-N.json`, `exit-N`, `trace-N.jsonl`, `check-N.txt` |

Board row: piece ID, title, state, worktree path, branch, provider and model
and effort of the latest attempt, attempt count, native session ID, pull
request, assumptions. States: `ready`, `running`, `checking`, `review`,
`waiting` (on CI or another piece), `parked` (on an inbox item), `done`,
`failed`.

Update the board before you start anything and after anything reports. If you
did not write it down, the next caller does not know it.

## Split the order into pieces

A piece is one change that can be checked on its own and lands as one pull
request or one artifact. For each piece write down:

- The finish line: something you can run or look up. A command that exits
  zero, a checklist of files that must exist, a pull request whose checks are
  green. "The worker says it is done" is not a finish line.
- What it depends on. Pieces with no dependency between them and no shared
  files run at the same time.
- A workspace of its own. For code, one git worktree and branch per piece:
  `git worktree add PATH -b BRANCH origin/MAIN`. Two workers never write to
  the same checkout.
- Provider, model, and effort for the first attempt. Start with the cheaper
  setting that has worked for this kind of piece. Use medium effort or higher
  for reviews. Keep the stronger model and the other provider for retries and
  for review.

Follow the repository's own workflow for issues, branches, and pull requests.
If it asks for a draft pull request before work starts, open it when you
create the worktree.

## Write the assignment

A worker cannot ask you anything during a run. Everything it needs is in
`assignment.md`, and the prompt is the working instructions followed by the
assignment (see "Give workers consistent instructions" in
`due skills show overview`). Include:

1. The task, the files or area, and the finish line command.
2. What it may change and what it may not. Workers edit, build, and test in
   their worktree and touch nothing outside it. Say whether the worker
   commits and pushes or leaves that to you, as the order decided. Workers do
   not merge, release, or delete remote state in either case.
3. This rule, in these words or close to them: "If you are unsure whether a
   choice is allowed and it can be undone, choose, list it under assumptions,
   and continue. Stop with `needs_decision` only when the choice cannot be
   undone or is outside this assignment. Stop with `blocked` only after you
   have tried another way."
4. "Run the finish line command before you report `done`, and list the
   commands you ran with their exit codes. Run each command by itself, with
   no pipes and no chaining." A permission allowlist matches whole commands,
   and a worker whose chained command is denied may report `blocked` without
   trying the plain command.

Request this result with `--schema`:

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["status", "summary", "assumptions", "questions", "checks"],
  "properties": {
    "status": {"enum": ["done", "blocked", "needs_decision"]},
    "summary": {"type": "string"},
    "assumptions": {"type": "array", "items": {"type": "string"}},
    "questions": {"type": "array", "items": {"type": "string"}},
    "checks": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["command", "exit_code"],
        "properties": {
          "command": {"type": "string"},
          "exit_code": {"type": "integer"}
        }
      }
    }
  }
}
```

Due passes the schema to the provider and does not validate the answer.
Validate `structured_output` yourself.

## Give workers room to work without approvals

A `due run` worker has nobody to approve a tool call, so set its permissions
in the profile and let anything outside them be denied. A denial is a result
you read, not a prompt that waits. Give each worker edits and the project's
build and test commands inside its worktree, and no network unless the piece
needs it. `due skills show profiles` has the exact shapes: for Codex,
`sandbox_mode="workspace-write"` with `approval_policy="never"`; for Claude,
`--permission-mode` with an `--allowedTools` list.

A worker also runs under the account's own provider settings: global
instruction files, hooks, and permission rules. They can change what a
profile does. A hook that rewrites commands, for example, makes an allowlist
entry for the original command stop matching. Before the first real piece on
a profile, run a one-line probe with it (`due run` with a cheap model, asking
the worker to run one allowed command and say whether it was permitted) and
read `permission_denials` in the result.

When the caller commits and pushes, none of the ask-first actions is within
a worker's reach. When the order has workers commit or push, the profile has
to allow it: the worktree's git directories must be writable (find them with
`git rev-parse --path-format=absolute --git-common-dir --git-dir`), and a push
needs network access and the worker's own credentials. The ask-first list
still applies, so say in the assignment that the worker pushes only its own
branch and never merges. Do not widen a worker's permissions to make a check
pass. If a piece needs more, decide that as the caller and record it on the
board.

## Start the runs

Save this as `$WORK/run-piece.sh`. It runs one attempt and leaves
`result-N.json`, `exit-N`, and `trace-N.jsonl` in the piece's directory. The
optional arguments are a different prompt file (a review or a retry) and a
native session ID to resume. Set `SCHEMA` to use another result schema.

```sh
#!/bin/sh
# usage: run-piece.sh ID PROFILE WORKTREE N [PROMPT_FILE] [RESUME_ID]
set -u
WORK=$(cd "$(dirname "$0")" && pwd)
id=$1 profile=$2 worktree=$3 n=$4
prompt=${5:-$WORK/pieces/$id/assignment.md}
resume=${6:-}
cd "$WORK/pieces/$id" || exit 2
status=0
if [ -n "$resume" ]; then set -- --resume "$resume"; else set --; fi
cat "$WORK/work-style.md" "$prompt" |
  due run --request "$WORK/profiles/$profile.json" --cwd "$worktree" \
    --stdin --output json --schema "${SCHEMA:-$WORK/result.schema.json}" \
    --timeout 30m --trace-file "trace-$n.jsonl" "$@" > "result-$n.json" || status=$?
printf '%s\n' "$status" > "exit-$n"
echo "piece $id attempt $n exit $status"
```

Start every `ready` piece whose dependencies are done, each as its own
background command, so that the pieces run at the same time and you stay free:

```sh
"$WORK/run-piece.sh" p1 claude-edit "$WORK/wt/p1" 1
```

Use the host's way of running a command in the background that reports when
the command exits. Claude Code continues the session when a background
command it started exits, so each run reports by itself and you do not poll.
On a host without that, start the runs with `&` and block on them with one
`wait`. Never sleep and poll in a loop, and never end your turn with pieces
`ready` and nothing running.

Always set `--timeout`. Record the piece as `running` on the board before the
command starts.

Use `due script` instead of `due run` when one worker should revise against
your check within a single process (`due skills show supervise`). Use a
configured Due MCP server when several workers share one workspace, for
example parallel read-only reviews. A server has one working directory, so
it does not replace a worktree per writing piece.

## When a run reports

Read `exit-N` and `result-N.json`, then act on the first row that applies:

| What you see | Do |
| --- | --- |
| Exit 0, `status` succeeded, result says `done` | Run the finish line yourself in the worktree and save the output as `check-N.txt`. Read the diff. If both hold, go to review or publish. If not, retry with the check output |
| Result says `needs_decision` | If you can decide under the rules above, decide, record the assumption, and resume the worker with the decision. Otherwise add an inbox item, mark the piece `parked`, and continue with other pieces |
| Result says `blocked` because a command was denied | Look at the worktree first. If the work is there and only the check did not run, run the finish line yourself and continue from the first row. Fix the profile or the assignment for later attempts. This is not a failed attempt |
| Result says `blocked` for another reason | Retry ladder |
| Exit 1, 124, or 130, or the result does not match the schema | Keep the files. Look at the worktree for partial edits. Retry ladder |
| `authentication` error, or a usage or rate limit | Do not retry on the same provider. Switch the piece to the other provider if the order allows it. If neither provider works, add an inbox item |
| Exit 2 or 127 | Your invocation is wrong. Fix the command or profile. This is not an attempt |

Copy the result's `session_id`, its assumptions, and the attempt's cost and
duration to the board. The worker's report is evidence in both directions: a
`done` can be wrong, and a `blocked` can sit on finished work. The finish
line and the diff decide.

## Retry ladder

Each rung is a new attempt with its own `N`. Give every attempt the earlier
failure as evidence (check output, the error, what the diff showed), marked
as evidence and not as new instructions.

1. Same worker, another approach: resume its session with `--resume ID` and
   the evidence. Use this once.
2. More effort or a stronger model, in a fresh session, with the assignment
   and the evidence. Reset or keep the partial edits deliberately and say
   which in the prompt.
3. The other provider, in a fresh session.
4. The inbox: what was tried, what each attempt showed, and what you
   recommend. Mark the piece `failed` and continue with the rest.

Stop climbing as soon as the evidence shows that the assignment, not the
worker, is wrong. Rewrite the assignment or split the piece instead.

## Review

Review is a separate fresh session that did not write the change. It gets
the issue or task, the diff, and read access to the worktree. For a hard
change use the other provider. For a hard change it is also worth having the
other provider check the plan before the author starts.

- Ask for findings with file and line, and a verdict.
- An approval from a reviewer that opened no file and ran no command does not
  count. Have the result list the commands run and files read, and compare
  that with the activity counts from `due trace summarize`. Run the review
  again at higher effort if they show nothing.
- A reviewer whose command for reading the change was denied has not seen
  it. Its verdict is not a review. Fix the profile and run it again.
- Check a finding before acting on it. Send confirmed findings to the author
  by resuming its session. Reject a finding only with evidence, and record
  the evidence.
- For a second round, give a fresh reviewer the new diff. A resumed reviewer
  tends to approve without looking. Tell the fresh reviewer what the earlier
  round raised and how each point was settled, including evidence you
  gathered since. Without that it raises the same points again.
- Set a limit on rounds, two unless the order says otherwise. After the last
  round, have the author apply confirmed findings that are small and local,
  run the finish line, and record them on the board. Take to the inbox only a
  disagreement you cannot settle with evidence.

## Publish and wait

When the finish line holds and review has passed, commit and push from the
piece's worktree, or confirm what the worker pushed if the order gave that to
workers. Follow the repository's rules, and open or update the pull request. Then start the wait as a background command, for example
`gh pr checks NUMBER --watch`, mark the piece `waiting`, and continue. When
the wait exits, a failed check goes back to the author as a retry with the
log as evidence. A passing check makes the piece `done` if the order stops
at a ready pull request, or creates an inbox item if merging is next.

## The inbox

One item per question or approval: what you need, the piece it blocks, the
options, and the one you recommend. Tell the person through the host's
notification or message channel when you add an item and when the order
finishes. Do not wait in the conversation for an answer. When an answer
arrives, record it in `order.md`, remove the item, and return the piece to
`ready`.

## Continuing after an interruption

If you start a turn and `$WORK/board.md` exists for the order, you are
continuing. Read `order.md`, `board.md`, and `inbox.md`. For each `running`
piece, check whether its result file exists and whether its process is still
alive, then handle it as a report or start the next attempt. Native session
IDs on the board let you resume workers. Nothing else from the earlier
conversation is needed.

## Finishing

The order is finished when every piece is `done`, `failed`, or `parked`.
Report once: each piece with its state and pull request, the assumptions you
made, the open inbox items, and what you could not verify. Remove the
worktrees of `done` pieces whose changes are pushed. Leave the rest in place
and name them in the report.
