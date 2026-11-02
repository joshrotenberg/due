---
name: run
description: Run one bounded Claude Code or Codex task through Due and independently verify its artifacts and effects.
---

# Run a task

`due run` wraps one noninteractive `claude -p` or `codex exec` invocation.
It is one foreground CLI run; the caller and OS own concurrency and scheduling.
Use the provider the user selected, with its installed CLI, authentication,
settings, and native permissions. Due does not grant additional authorization
or make the providers' permission policies equivalent.

For a bounded assignment that needs a resident session, streamed items,
permission decisions or independent verify/revise steps, use `due script` and
read the installed `supervise` guide. Raw `due rpc` also supports custom clients.
Use commands exposed by the installed build; `due skills` lists its guides.

Define the concrete deliverable, working directory, allowed changes, and stopping
condition in the prompt. Carry existing user authorization into the task without
expanding it. Inspect existing workspace changes when they matter to later verification.

`due run` has no default timeout. If you will watch the run's events and artifacts and
can cancel it, omit `--timeout`. Unattended permission settings (no approval
prompts) do not mean nobody is monitoring. If nobody will monitor the run, or the
task has a real time budget, choose a generous task-specific `--timeout`
deliberately. It is an absolute limit from provider start: progress does not
reset or extend it, and it cannot be extended mid-run. There is no universal value.

Send the prompt through stdin and request the result envelope for agent callers:

```sh
due run --provider codex --cwd "$PWD" --stdin --output json <<'PROMPT'
Inspect this repository without changing files. Identify the failing test's
likely cause and report the relevant files and evidence. Stop after diagnosis.
PROMPT
```

For an unmonitored run with an explicit time budget, add a limit such as
`--timeout 45m`, sized to the task.

Replace the example scope with the actual assignment. `--provider claude` uses
the same shared options. Omit `--model` and `--effort` to retain native defaults;
model names and supported effort levels remain provider-specific. These are the
new CLI session's settings, not the calling agent's model, conversation, or
per-turn permission grants. Include the necessary context in the prompt.

Due starts one foreground provider invocation; it does not disable native
subagents. State whether delegation is in scope. To disable built-in subagent
tools for a task, use Codex passthrough `-c agents.enabled=false -c features.multi_agent_v2=false` or Claude
`--disallowedTools Agent`, alongside the task's other native permissions. These
controls do not prevent every possible subprocess launch.
With execution flags, exactly one of `--stdin` and `--prompt TEXT` is required. Both reach the child
through stdin, but `--prompt` exposes the prompt in Due's own argv.

A request file can hold a complete task or a partial profile. Only
`schema_version: 1` is required in the file; provider and prompt must be supplied
after merging. Explicit CLI execution flags replace file values; absent flags
inherit them. For example, save this as `profile.json`:

```json
{"schema_version":1,"provider":"codex","ephemeral":true}
```

A profile `timeout` applies whenever `--timeout` is omitted. To run without it,
remove the field or set it to `null`.

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
whole profile list, including permission grants and restrictions;
`--clear-native-args` clears it. Paths for cwd and schema use
the caller's directory. `--request -` reads a whole JSON document from stdin and
cannot share stdin with `--stdin`; a CLI `--prompt` can override that document.
Changing provider does not reset inherited policy or other options; review them
for the selected provider.

Add `--trace` for readable progress, or load `due skills show observe` for JSONL
monitoring. Load `due skills show contracts` when the caller needs structured
answers or must interpret result fields. `due skills` lists the available skills;
`due skills list --output json` provides the catalog for automated selection.
`due skills show NAME --output json` also returns a machine-readable response.

Use `--dry-run` to validate the merged task, native conflicts, cwd, and schema
JSON without launching a provider. It works without an installed provider and
does not verify authentication, model availability, native CLI option/config
support, effective sandbox policy, or provider schema support.
It omits prompt text but shows native option values and the inline Claude schema;
requested tracing and temporary schema snapshots may still write files.
Pass native options after `--`, for example `-- --max-turns 5` for Claude.
Use shared flags for cwd, prompt transport, output, and session selection.
Passthrough accepts options, not another command or positional prompt. For an
unknown option whose value begins with `-`, use `--option=value`.
Native permissions remain in force; do not add bypass flags as a default.

If execution fails, preserve results, stderr, traces and partial edits before any
deliberate follow-up. Separate host launch/PATH failures, native state/keychain
access, actual login status, task sandbox denials, Git/resume mismatches and native
schema rejection. Check provider status in the same normal host context; a
sandbox-only `Not logged in` does not prove expired login. If that normal-host
status requires login, the human must complete interactive login there. Do not
copy credentials, widen grants or automatically retry. Source installations have
[recovery and existing-build guidance](../../docs/troubleshooting.md).

For persistent unattended Codex work, save this exact JSON as
`codex-workspace-profile.json` in a caller-owned location outside every worker
writable root. Use it unchanged for fresh and explicitly resumed runs; per-run
inputs remain with the caller. No source checkout is needed. An optional source
copy is [here](../../examples/codex-workspace-profile.json).

```json
{
  "schema_version": 1,
  "provider": "codex",
  "ephemeral": false,
  "native_args": [
    "-c", "sandbox_mode=\"workspace-write\"",
    "-c", "approval_policy=\"never\"",
    "-c", "agents.enabled=false",
    "-c", "features.multi_agent_v2=false",
    "-c", "sandbox_workspace_write.network_access=false",
    "-c", "sandbox_workspace_write.writable_roots=[]",
    "-c", "sandbox_workspace_write.exclude_slash_tmp=true",
    "-c", "sandbox_workspace_write.exclude_tmpdir_env_var=true"
  ]
}
```

Use config-form sandbox mode: Codex CLI 0.162.0 exposes `-c` on both paths but
rejects `--sandbox` on `exec resume`, even if Due's dry run succeeds.

Use an existing authorized Git checkout, or initialize an authorized disposable
repository first. The normal Codex Git/trust check stays enabled; the preflight
below does not override it. Never add `--skip-git-repo-check` automatically.
Any authorized new native option belongs in a copy of the **full** profile list;
passing it alone after `--` replaces all eight settings.

Use a canonical workspace path (`pwd -P`) for native grants. If the outer host
rejects symlinked writable roots before process creation, report that layer and
request a supported host workspace/permission correction; do not change broad
trust settings automatically.

Choose an existing caller-writable `artifact_root` outside every worker writable
root. The Due commands below set `TMPDIR` to that protected artifact
directory for their schema snapshot and final-message file. The profile excludes
that directory's temporary write grant. Do not export workspace-local `TMPDIR`
to Due. If tests need temporary files, tell the task to set `TMPDIR` only
on its test commands, for example `TMPDIR="$PWD/test-tmp" python3 -m unittest`
from the task cwd. Worker commands inherit the protected `TMPDIR` and cannot
write there; set a workspace-local value on any command that needs temporary
files. Do not widen worker grants.

This example uses `jq`, the saved profile, and a caller-supplied answer schema
also kept outside worker writable roots. Keep the separate bounded prompts
`first-task.txt` and `follow-up.txt` in that caller-owned inputs directory too:

```sh
input_root=/absolute/path/to/caller-owned-inputs
profile="$input_root/codex-workspace-profile.json"
answer_schema="$input_root/answer.schema.json"
workspace=/absolute/path/to/authorized-git-checkout
artifact_root=/absolute/path/to/caller-owned-artifacts-outside-workspace
workspace=$(cd "$workspace" && pwd -P) || exit 1
git -C "$workspace" rev-parse --git-dir > /dev/null || exit 1
artifacts=$(mktemp -d "$artifact_root/due-run.XXXXXX") || exit 1
mkdir -p "$workspace/test-tmp" || exit 1
fresh_status=0
TMPDIR="$artifacts" due run --request "$profile" --cwd "$workspace" --stdin --output json \
  --schema "$answer_schema" --trace --trace-file "$artifacts/fresh.jsonl" \
  < "$input_root/first-task.txt" > "$artifacts/fresh.result.json" || fresh_status=$?
printf '%s\n' "$fresh_status" > "$artifacts/fresh.exit"
[ "$fresh_status" -eq 0 ] || exit "$fresh_status"
```

Before continuing, the calling agent independently validates `structured_output`
against `answer_schema` and checks the intended task, actual cwd, permissions,
and effects. Native session data is optional extra evidence when accessible.
These checks can be automated under existing authorization; no human approval
or manual ID paste is required. Due does not validate the answer or effective
policy. Report missing evidence and blocked checks as such. Use the same
workspace: native resume can restore its workspace, and `--cwd` is not a proven
way to relocate history.

After those checks, correlate this fresh result with its trace and set the
verified ID programmatically. This trace-enabled recipe requires a session
marker and completion; investigate missing evidence before follow-up:

```sh
session_id=$(jq -er '
  select(.provider == "codex" and .status == "succeeded" and
         .structured_output_present == true) |
  .session_id | select(type == "string" and length > 0)
' "$artifacts/fresh.result.json") || exit 1
run_id=$(jq -er '.run_id | select(type == "string" and length > 0)' \
  "$artifacts/fresh.result.json") || exit 1
jq -es --arg run "$run_id" --arg id "$session_id" '
  length > 0 and .[0].event == "run_started" and
  .[-1].event == "run_finished" and .[-1].status == "succeeded" and
  .[-1].exit_code == 0 and
  all(.[]; .run_id == $run and .provider == "codex") and
  any(.[]; .event == "session_started") and
  all(.[] | select(.event == "session_started" or has("session_id"));
      .session_id == $id)
' "$artifacts/fresh.jsonl" > /dev/null || exit 1
verified_session_id=$session_id
printf '%s\n' "$verified_session_id" > "$artifacts/native-session-id.txt"
resume_status=0
TMPDIR="$artifacts" due run --request "$profile" --cwd "$workspace" --resume "$verified_session_id" \
  --stdin --output json --schema "$answer_schema" \
  --trace --trace-file "$artifacts/resumed.jsonl" \
  < "$input_root/follow-up.txt" > "$artifacts/resumed.result.json" || resume_status=$?
printf '%s\n' "$resume_status" > "$artifacts/resumed.exit"
[ "$resume_status" -eq 0 ] || exit "$resume_status"
jq -e --arg id "$verified_session_id" '
  .provider == "codex" and .status == "succeeded" and
  .structured_output_present == true and .session_id == $id
' "$artifacts/resumed.result.json" > /dev/null || exit 1
```

Each run gets a new prompt and artifacts. Preserve its execution status before
summarizing traces or validating answers. Due has no implicit last-session
selection or session database. Caller acceptance on Due 0.1.0 / Codex 0.162.0 /
macOS 27 arm64 used the earlier seven-setting profile for two same-workspace turns: session
continuity, remembered context, edits/tests, and actual denied outside writes
were checked. Native contexts exposed workspace-write, approval never, network
access false, and both excluded temp grants true. All seven original settings were
forwarded, but effective `agents.enabled` and explicit `writable_roots` were not
exposed; no observed delegation does not confirm agent disabling. The profile now
also disables `features.multi_agent_v2`, which takes precedence over
`agents.enabled` in the version-matched native source. This static check is
separate from live tool-availability evidence. Confirm native
settings where possible and report unconfirmed or unsupported values without
silently removing them. This is bounded host/version evidence, not a general
security guarantee; synthetic tests establish routing only. Due neither enforces
nor validates the sandbox independently. The optional
[full recipe](../../docs/cli.md#reusable-codex-workspace-profile) has more detail.

This grants workspace edits and sandboxed commands, not only one file or command.
It requests no extra writable roots or default temporary-directory grants; additional
paths require explicit authorization. Put test temporary files inside the
workspace only for the task's test commands. Reads can extend outside the workspace.
Command networking is disabled; provider API and connected-tool permissions are
separate. Inherited native rules and managed requirements still apply. Preserve
them and report blocked checks without retries or expanding permission grants.
Keep the task's narrower scope in the prompt and independently verify effects.
Due traces omitted the acceptance test's denied write as a failed activity
(known issue #6); native session tool output and unchanged canaries established
the denial. Raw native exec JSON before parsing was not checked in that initial
batch. A later Codex 0.162.0 correlated capture found one inline Python denial
in native session tool output but no corresponding command event in native
stdout before Due parsing. Other script controls did emit failure events; these
versioned observations do not prove coverage of all denials. Trace completeness
alone therefore does not prove that checks were allowed.

For unattended Claude work, `--tools` exposes built-in tools but does not
authorize their use. `acceptEdits` permits edits, not arbitrary shell commands. Grant only the
edits and checks authorized for this task, using native permission rules:

```sh
due run --provider claude --cwd "$PWD" --stdin --output json \
  -- --permission-mode dontAsk --permission-prompts none \
  --tools Read,Edit,Bash \
  --allowedTools 'Edit(/receipt.py)' 'Bash(python3 -B -m unittest -v)' < task.txt
```

This example grants one file edit and one test command. Replace both with the
actual authorized scope, and tell the worker to run the exact command from cwd.
The leading slash in this CLI Edit rule anchors it to the starting directory.
`dontAsk` denies actions that need further approval; normal permitted reads and
inherited rules still apply. Deny/ask rules override allows. The optional
`--permission-prompts none` requires Claude Code 2.1.259+ and tells the worker not
to retry unresolved approvals. Check the installed provider's help for support.
Store the same tokens in a profile's `native_args` for repeatable use. Supplying
arguments after `--` replaces that entire list, including permission rules.
This supplements native policy; it does not sandbox repository code executed by
an approved test command. Do not change global policy to make a run pass.
If a check is denied, report it as blocked even if the Due execution succeeds.

Use `--resume ID` only for the intended provider's session and authorized follow-up.
It requires a new prompt. `--ephemeral` disables persistence and conflicts with
resume; an ID returned by an ephemeral run may not be resumable.
Persistence is allowed by default. If follow-up is likely, omit `--ephemeral`
(or override a profile with `--no-ephemeral`) from the first run and save its
native `session_id`. Due does not keep its own session store or resume a nested
child agent directly. Disabling ephemeral mode later cannot recover lost context.

After the run, inspect its exit status and result. For structured answers, require
`structured_output_present` and independently validate the value when correctness
is a gate; `answer_schema_validation` does not claim independent validation.
Then inspect the actual
deliverables: changed files, diffs, generated artifacts, relevant checks, or the
external system the task was authorized to change. A provider's narrative and
activity records are reported evidence; label effects as verified only after
independent observation. Preserve unrelated workspace changes.

To stop a run you are monitoring, send Due SIGINT, SIGTERM, or SIGHUP; it cancels
the provider cooperatively. Decide from observed events and artifacts, not from
heartbeats alone (`due skills show observe`).
Do not retry automatically after failure, timeout, or cancellation. Report the
partial outcome and inspect possible effects before any authorized follow-up.
Due does not roll back changes or collect a verified side-effect inventory.
