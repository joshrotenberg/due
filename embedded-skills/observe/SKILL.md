---
name: observe
description: Observe a Due run through content-limited progress traces while preserving final results and detecting trace loss.
---

# Observe execution

Enable tracing when live session, process, activity, or waiting information helps.
Tracing changes observation, not the task's authorization or native permissions.
For a prepared prompt in `task.txt`, choose a new log path and run:

```sh
due run --provider claude --stdin --output json --timeout 5m \
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

Traces omit prompts, schemas, argv, paths, assistant text, reasoning, tool inputs,
tool results, and raw diagnostics. They retain restricted IDs/tool names and counts.
Stderr tracing suppresses native diagnostics; detailed errors remain in the final
JSON result and may contain provider content. With file-only tracing, ordinary
stderr diagnostics remain enabled, except successful Codex stderr is unavailable
through the current wrapper streaming API.

Ctrl-C and `--timeout` await wrapper cleanup. On Unix, SIGINT asks the provider to
stop its tool sessions; output drains during a two-second grace, followed by a
forced kill of the remaining owned group and direct-child reaping. Detached tool
cleanup depends on the provider's cooperation; group kill alone cannot reach it.
Cleanup can extend beyond the deadline. Cancellation is not rollback.
Inspect artifacts and authorized external effects independently after interruption
or completion; trace events describe observations, not a verified effect inventory.
Do not retry automatically. Use `due skills show contracts` to interpret the
final result, or `due skills show run` to scope an authorized follow-up.
