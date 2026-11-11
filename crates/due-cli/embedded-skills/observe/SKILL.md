---
name: observe
description: Observe a Due run through content-limited progress traces while preserving final results and detecting trace loss.
---

# Observe execution

This guide describes `due run` progress traces. For resident assignments,
`due script` provides streamed session items and process/command evidence;
read `due skills show supervise` in a harness-enabled build. These full items
can contain task content and are not the content-limited trace format below.

Enable tracing when live session, process, activity, or waiting information helps.
Tracing changes observation, not the task's authorization or native permissions.
For a prepared prompt in `task.txt`, choose a new log path and run:

```sh
due run --provider claude --stdin --output json \
  --trace=json --trace-file run.jsonl --trace-interval 1s < task.txt
```

The same flags work with Codex. Stdout contains one final Due result; JSONL progress
goes to stderr and the optional file. Parse each channel separately. Bare `--trace`
selects readable stderr instead; use `--trace=json`, not `--trace json`.
`--trace-file` alone writes JSONL without printing progress on stderr. Files are
created exclusively, with mode 0600 on Unix; an existing path fails before spawn.
Relative trace paths resolve from the caller's directory.

Correlate records by `run_id`, then `seq`; use `session_id`, `pid`, and Unix `pgid`
when available. Records carry `schema_version`, `provider`, `event`, timestamps,
elapsed time, and observed counters. A heartbeat reports waiting and `idle_ms`;
it is a liveness signal, not proof of work or a completion percentage.
The default heartbeat interval is 5s, beginning after prompt input is complete.

Heartbeats show only that Due is alive and waiting. Native silence can be
legitimate reasoning, a long tool call, or missing provider telemetry; frequent
activity can still be repetitive. Due has no idle watchdog. Judge progress from
observed events and the artifacts the task produces, and cancel deliberately
rather than automatically from `idle_ms`. A run you actively monitor needs no
`--timeout`; Due has no default. For an unmonitored run, choose a generous
task-specific `--timeout`. It is absolute and does not extend with activity.

`activity_started` and `activity_completed` carry an `activity` category:
`command`, `file_change`, `mcp_call`, `web_search`, `plan_update`, `status_update`,
`tool_call`, or `unknown`. Completion `status` is `succeeded`, `failed`,
`cancelled`, or `unknown`. Correlate available `activity_id` values, but tolerate
missing starts/completions and future categories. Providers expose different
events; counts are not equivalent units of work. A failed activity can be
followed by a successful turn. Claude content-block completion alone does not
mean that a tool finished; Due waits for the native tool result.

Use the final process exit and result to determine the run outcome.
`run_finished` has `scope: execution`: it describes execution before final stdout
delivery, which can still fail. Check result `trace.incomplete` and
`trace.dropped_records`; sequence gaps also indicate lost observation.
Queues are bounded and shutdown drains for at most two seconds. Sink failure or
slow consumers can lose records, including the terminal event, without changing
the provider outcome. Do not infer success or failure from missing trace records.

Default traces omit prompts, schemas, argv, paths, assistant text, reasoning, tool inputs,
tool results, and raw diagnostics. They retain restricted IDs/tool names and counts.
Stderr tracing (text or JSON) suppresses native diagnostics and Due's error-message
text; the trace retains error kinds and exit codes. Use `--output json` for detailed
errors, which may contain provider content. With file-only tracing, ordinary
stderr diagnostics remain enabled, except successful Codex stderr is unavailable
through the current wrapper streaming API.

For additional visibility, `--trace-detail` opts into bounded command/file/tool
labels and user-facing assistant messages where the provider exposes them. It
requires `--trace` or `--trace-file`. Details may include paths, arguments, or
provider content; reasoning/thinking, prompts, and raw tool results are excluded.
Truncated details are marked, and text rendering escapes terminal controls.
Default traces remain metadata-only. Claude may emit an `activity_started`
record before it supplies the requested Bash command in a later
`activity_detail` record with the same `activity_id`. A detail with
`source: "permission_check_input"` comes from a native denial and may show a
rewritten command. Claude can complete a tool call without exposing a hook's
final command, so the requested label is not execution evidence.

Summarize an existing JSONL log offline:

```sh
due trace summarize run.jsonl
due trace summarize run.jsonl --output json
due schema trace
due schema trace-summary
```

The summary groups multiple runs and surfaces observed outcomes, counters,
sequence gaps, missing boundaries, and unmatched activities. Unknown event kinds
are retained as observations. Missing terminal records leave the outcome unknown;
never infer an execution failure from log incompleteness. The summarizer's exit
status reports whether it read/parsed the log, not the recorded agent outcome.
Malformed or oversized records fail; no partial summary is reported as complete.
Retain the final result's delivery metadata too: a log cannot reveal every sink loss.

Ctrl-C and `--timeout` await wrapper cleanup; Unix SIGTERM and SIGHUP use the same
cancellation path. The provider receives SIGINT to
stop its tool sessions; output drains during a two-second grace, followed by a
forced kill of the remaining owned group and direct-child reaping. Detached tool
cleanup depends on the provider's cooperation; group kill alone cannot reach it.
Cleanup can extend beyond the deadline. Handled signals exit 130; timeouts exit 124.
SIGKILL cannot be handled and may leave provider/tool processes running; prefer
SIGINT/SIGTERM/SIGHUP or `--timeout` to an external hard kill. Cancellation is not rollback.
Inspect artifacts and authorized external effects independently after interruption
or completion; trace events describe observations, not a verified effect inventory.
Do not retry automatically. Use `due skills show contracts` to interpret the
final result, or `due skills show run` to scope an authorized follow-up.
`due run --help` lists the trace flags. For resident sessions, items arrive as
`session.item` records (`due skills show rpc`) or through `onItem` in `due script`
(`due skills show supervise`); `due skills show overview` maps the guides.
