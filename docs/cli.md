# Due CLI reference

Detailed operating rules for `due run`, request profiles, structured answers,
tracing, native passthrough, and results. Start with the [README](../README.md)
for the overview and quickstart. The installed binary is the authority for its
own version: `due run --help`, `due skills show NAME`, and `due schema NAME`
describe exactly what that build accepts. For resident sessions see the
[JavaScript harness](harness.md) and [raw RPC](rpc.md) references.

Each `due run` invocation owns one foreground provider run; the caller and operating
system own concurrency, scheduling, and any later retry.

## Embedded guidance and contracts

Detailed guidance ships inside the executable, so it stays available alongside
the installed behavior without a source checkout, provider process, or network:

```sh
due -h                             # Short help: commands in this build
due --help                         # Long help: examples and cross-references
due run --help                     # Every subcommand has short -h and long --help
due skills                         # Names and short descriptions
due skills list --output json      # Versioned catalog for automated selection
due skills show overview           # Choose a mode, set up, troubleshoot, report
due skills show run                # Scope, execution, and verifying effects
due skills show profiles           # JSON profiles across run, script and RPC
due skills show contracts          # Structured answers and result semantics
due skills show observe            # Progress, trace loss, and cancellation
due skills show rpc                # Raw stdio client wire reference
due skills show supervise          # Resident JavaScript assignments
due skills show contracts --output json
due schema request                # Installed input contract as JSON Schema
due schema result                 # Installed result envelope contract
due schema trace                  # Installed progress record contract
due schema trace-summary          # Installed offline summary contract
due schema script-result          # Installed JavaScript result contract
due trace summarize run.jsonl --output json
```

`skills show` prints a complete Agent Skills `SKILL.md` by default. JSON catalog
and document output include `schema_version: 1` and `cli_version`; unknown names
return a structured `invalid_input` error and exit 2 in JSON mode. Normal argument
syntax errors use stderr and exit 2. These commands are read-only and do not need
a provider installation. The canonical instructions live in
[`embedded-skills/`](../crates/due-cli/embedded-skills) and are included at compile time;
changing them requires rebuilding the binary.

Short `-h` output lists commands and options compactly; `--help` adds examples
and detail. The `rpc` and
`supervise` guides appear only in builds that include `due rpc` and `due script`;
use `due -h` and `due skills` to see what an installed binary offers. Installed
guides are self-contained, so a source checkout or website is not required.

The entry skill stays small; load only the detailed guide relevant to the task.
The current contract concerns execution and returned data. Artifact and external
side effects still require the caller's independent verification.

### Agent plugin

`due init --dry-run` previews the bundled plugin's installation; `due init` applies
it for both hosts, or use `--host claude` / `--host codex` for one. The selected
host CLI must be installed. Setup writes a managed local marketplace and uses
native plugin commands; it starts no model work and does not change the binary,
permissions, authentication or caller policy. `--output json` returns a separate
versioned setup report. See [installation and recovery](install.md#install-the-bundled-plugin)
for paths, partial failures, update and migration rules.


The skills-only plugin in [`plugin/due`](../plugin/due) provides two short
entry skills: [`delegate`](../plugin/due/skills/delegate/SKILL.md) and
[`work-loop`](../plugin/due/skills/work-loop/SKILL.md), for orders larger
than one assignment. Each routes to the guides in the binary; the `work-loop`
procedure is `due skills show work-loop`. The `delegate` skill frames Due as an
independent worker and starts an agent with `due -h`, `due --help`,
`due COMMAND --help`, `due skills list --output json` and `due skills show NAME`. The plugin requires local command
execution, Due on PATH, and the selected provider CLI installed and authenticated.
It follows the [Agent Plugins package format](https://agent-plugins.org/plugin-authors/build-an-agent-plugin).
The package does not install Due or a provider and does not change the caller's
authorization. See [install.md](install.md) for installation steps.

## Build dependencies

Due uses wrapper versions with passthrough support
([claude-wrapper #817](https://github.com/joshrotenberg/claude-wrapper/pull/817),
[codex-wrapper #148](https://github.com/joshrotenberg/codex-wrapper/pull/148)) and
cooperative shutdown
([claude-wrapper #818](https://github.com/joshrotenberg/claude-wrapper/pull/818),
[codex-wrapper #149](https://github.com/joshrotenberg/codex-wrapper/pull/149)).
Both wrappers are pinned to exact crates.io releases. Keep `Cargo.lock`
committed for reproducible application builds.

## Shared options

| Option | Behavior |
| --- | --- |
| `--request FILE\|-` | Read a versioned JSON request or partial profile; explicit CLI values override it. |
| `--provider claude\|codex` | Select the native CLI; required after merging request and flags. |
| `-p, --prompt TEXT` | Prompt text, replacing any profile prompt; conflicts with `--stdin`. |
| `--stdin` | Read the entire UTF-8 prompt, replacing any profile prompt; conflicts with `--request -`. |
| `--model ID` | Native model name/alias. Omitted values inherit the profile, then the provider default. |
| `--effort low\|medium\|high` | Native effort level. Support and computation vary by provider/model. |
| `-C, --cwd DIR` | Child working directory, resolved from the caller's cwd. |
| `--timeout 30s` | Absolute execution limit; supports durations such as `5m`. No default. See [deadlines and monitoring](#deadlines-and-monitoring). |
| `--output text\|json` | Final answer text by default, or one versioned JSON result. |
| `--schema FILE` | Request a structured answer using a JSON Schema; preserve it as a JSON value in the result. |
| `--resume ID` | Continue a native session/thread with the supplied prompt. |
| `--ephemeral` / `--no-ephemeral` | Explicitly disable / allow native session persistence, overriding the profile. Ephemeral conflicts with resume. |
| `--clear-native-args` | Replace profile native arguments with an empty list. Conflicts with arguments after `--`. |
| `--dry-run` | Validate the merged request and show executable, argv, cwd, and stdin size without launching a provider. |
| `--binary PATH` | Override the selected provider's executable. |
| `--trace[=text\|json]` | Live progress on stderr; bare `--trace` uses readable text. |
| `--trace-file PATH` | Create a new JSONL progress log; can be used with or without stderr tracing. |
| `--trace-interval 5s` | Heartbeat period while tracing, after reading the prompt. Must be positive. |
| `--trace-detail` | Opt into bounded command/file/tool labels and assistant messages when a trace sink is enabled. May contain provider content. |

Prompts from `--prompt`, `--stdin`, or a request file must be at most 1 MiB.
Larger prompts are rejected as invalid input (exit status 2).

Prompts always travel to the child through stdin. A prompt supplied on Due's own
command line is still visible in Due's argv; use `--stdin` when that matters.

When a comparison needs exact native CLI identity, pass `--binary` a verified
versioned executable and record its resolved path, version and SHA-256 at launch.
A mutable PATH alias can change mid-batch
([#54](https://github.com/joshrotenberg/due/issues/54),
[#75](https://github.com/joshrotenberg/due/issues/75)). Do not update provider
installations automatically.

Keep provider executable identity separate from Due build identity and from
requested, configured and provider-reported model or effort; unreported values
stay unknown. See the [reporting guide](dogfooding.md#keep-enough-evidence).
Enhancement tracking remains [#1](https://github.com/joshrotenberg/due/issues/1)
and [#2](https://github.com/joshrotenberg/due/issues/2).

### Dry runs

Dry runs validate input shape, the merged task, native-option conflicts, working
directory, and answer-schema JSON. They omit prompt contents and work even when
the provider is not installed. They do not check authentication, model support,
native option compatibility, or provider-specific schema constraints. A dry run
may create an explicitly requested trace file and a temporary schema snapshot.
Provider options themselves are shown in dry-run output, including an inline
Claude schema, so secrets in native options or schemas are not redacted.

### Process lifetime and cancellation

Working-directory and executable paths must be valid UTF-8 so JSON results can
represent the actual invocation without lossy path conversion.

Relative executable paths and PATH lookups resolve before `--cwd` changes the
child's directory. The execution timeout starts when the provider runs, after the
complete prompt has been read. On Unix, Ctrl-C, SIGTERM, SIGHUP, and timeouts ask
the native CLI to interrupt with SIGINT, while the wrappers drain its output for a two-second grace
period. They then force-stop the remaining owned process group and reap the direct
child before Due returns. Shutdown may extend beyond the execution deadline.
Native interruption lets the CLI clean up tool processes in separate groups;
forced group termination alone cannot reach those processes. Cleanup of escaped
processes still depends on native cooperation. Other platforms use the wrappers'
available process cleanup. Due performs no automatic retries.

Handled signals produce a cancelled result and exit 130 after cleanup; timeouts
exit 124. SIGKILL cannot be handled and may leave provider/tool processes running.
Prefer Due's cooperative cancellation (`--timeout` or SIGINT/SIGTERM/SIGHUP) to an
external hard kill; the trace's pid/pgid can help inspect processes after an
abrupt stop.

### Deadlines and monitoring

Due has no default execution timeout. When a caller actively watches the run's
events and artifacts and can cancel it, omit `--timeout` and any profile
`timeout`. Unattended permission settings (no approval prompts) describe the
provider's permissions, not whether someone is monitoring the run.

For a run nobody is monitoring, or one with an actual time budget, choose
a generous, task-specific `--timeout` deliberately. It is an absolute limit from
provider start: progress does not reset or extend it, and it cannot be extended
during the run. There is no recommended universal value.

```sh
# Explicit time budget for an unmonitored run.
due run --provider codex --stdin --output json --timeout 45m < task.txt
```

A profile's `timeout` applies whenever the CLI omits `--timeout`. To run without
it, remove the field or set it to `null` in the file.

Heartbeats show that Due is alive and waiting; they do not show whether the
provider is making progress. Native silence can be legitimate reasoning, a long
tool call, or missing provider telemetry, and frequent activity can be
repetitive. Due has no idle watchdog. Inspect observed events and artifacts
rather than cancelling or extending automatically from heartbeats. To stop a
run, send SIGINT, SIGTERM, or SIGHUP to Due, then inspect the partial effects it
may have left before any follow-up.

### Sessions

Resume IDs stay provider-specific. `session_id` in Due's result is Claude's native
session ID or Codex's native thread ID. Ephemeral runs may report an ID that cannot
be resumed. There is no local session database or implicit "latest" selection.
`--resume ID` requires a new prompt and conflicts with `--ephemeral`. Allow
persistence from the first turn; `--no-ephemeral` cannot restore history that an
earlier ephemeral run did not save. Codex can restore a session's native
workspace on resume. Verify the intended session and workspace before follow-up;
Due's `--cwd` sets the child process directory and does not promise to relocate
historical native sessions. See the [persistent profile recipe](#reusable-codex-workspace-profile).

## JSON requests and profiles

A request file supplies defaults for one invocation. Explicit execution flags
replace its values; omitted flags retain them. This supports both complete tasks
and reusable partial profiles:

```json
{"schema_version":1,"provider":"codex","ephemeral":true}
```

```sh
due run --request examples/profile.json --stdin --output json < task.txt
due run --request examples/profile.json --provider claude -p "Review the diff" \
  --dry-run --output json
due run --request task.json --timeout 30m --output json   # explicit time budget
due run --request task.json --no-ephemeral --resume native-session-id \
  --stdin --output json < follow-up.txt
cat task.json | due run --request - -p "Use this replacement prompt" --output json
```

`due skills show profiles` covers the same files for `due run`, `due script` and
raw RPC, including the narrower native options resident sessions accept.

Only `schema_version: 1` is required in the file. A provider and non-whitespace
prompt must be present after merging, or use `--stdin` for the prompt.
The [request schema](../crates/due-cli/schemas/run-request.schema.json) describes the profile's
structural shape; `due schema request` exports the installed version offline.
The [full request](../examples/run-request.json) and [profile](../examples/profile.json)
are examples. There is no profile search path, inheritance chain, or config store.

| Field | Accepted final value or default |
| --- | --- |
| `provider` | `claude` or `codex`; may be omitted but cannot be null. |
| `prompt` | Non-whitespace literal string; may be omitted but cannot be null. |
| `model` | Nonempty native model string; omitted or null keeps the provider default. |
| `effort` | `low`, `medium`, `high`, or null. |
| `cwd` | Working directory string or null; defaults to the caller's directory. |
| `timeout` | Positive duration string such as `30s` or `5m`, or null for no deadline. Omitting `--timeout` inherits it. |
| `schema` | Answer-schema filename string or null. |
| `resume` | Nonempty native session ID string or null. |
| `ephemeral` | Boolean, default false; true conflicts with a non-null `resume`. |
| `native_args` | Ordered array of strings, default empty. |

Unknown or duplicate keys, unsupported versions, malformed JSON, and wrong JSON
types fail before merging, even if a flag would replace the field. Semantic
checks apply after merging: for example, `--timeout 2m` can replace a profile's
invalid duration string. The request schema therefore documents structure;
`--dry-run` also validates the resolved task. Duration parsing is not approximated
with a schema regex.

`--ephemeral` and `--no-ephemeral` explicitly override the profile's boolean.
Native arguments after `--` replace the **entire** `native_args` list; they do not
append. `--clear-native-args` provides an empty replacement. A bare `--` inherits
the list. Replacement removes permission grants and restrictions too; supply the
complete intended list if replacing it. Switching providers keeps inherited
policy and other values, which must remain valid for the selected provider;
Due does not silently discard options.

`--prompt` or `--stdin` replaces the profile prompt. `--request -` reads all stdin
as JSON, so it cannot be combined with `--stdin`; use a file or `--prompt` instead.
Request `cwd` and `schema` paths resolve from the caller's directory, not the
request file's directory. Native argument paths follow the native CLI's rules.
`schema` names the model-answer schema, distinct from Due's request schema.

`--output`, `--dry-run`, `--binary`, and tracing flags are local CLI controls,
not request fields. Output still defaults to text. Request intake errors in JSON
mode use the result envelope with `provider: null`, `error.kind: invalid_input`,
and exit 2. After resolution, runtime errors identify the selected provider.
Clap syntax errors use stderr and exit 2. Intake precedes tracing, so intake
failures may print a plain diagnostic even with `--trace=json` and create no log.

## A calling-agent workflow

Save a bounded assignment as `task.txt`, choose a fresh trace path, then validate
and execute the same merged input:

```sh
due run --request examples/profile.json --stdin --dry-run --output json \
  < task.txt > plan.json
due run --request examples/profile.json --stdin --output json \
  --trace --trace-file run.jsonl < task.txt > result.json
run_status=$?
due trace summarize run.jsonl --output json > trace-summary.json
# Retain run_status alongside result.json; the summarizer has its own exit status.
```

Inspect the run exit/status first. If the task requests structured data, check
`structured_output_present` and independently validate `structured_output` against
the answer schema. Then inspect the actual diff/artifacts and run the relevant
checks. The trace explains observed activity; neither it nor a successful answer
proves the changes are correct. A reviewer is a separate explicit Due invocation
with its own prompt, result, and trace. The caller owns any follow-up or repair.

## Structured answers

Optional caller examples cover [offline answer validation](../examples/offline-validation/README.md)
and [native schema smoke probes](../examples/native-schema-smoke/README.md).
They keep validation and compatibility evidence outside Due's execution status.

`--output json` serializes Due's result envelope. `--schema FILE` additionally
asks the model for an answer with a particular structure:

```sh
due run --provider claude -p "Summarize this repository" \
  --schema examples/summary.schema.json --output json
due run --provider codex -p "Summarize this repository" \
  --schema examples/summary.schema.json --output json
```

Both produce the same result fields, including `structured_output` as a JSON
value. The common schema option also works with `--resume`. `text` remains the
native final answer; for compatibility, an empty successful Claude answer uses
the serialized structured value as its text fallback. Native structured Claude
data is retained even without `--schema`, and even when accompanying nonempty
text. Codex text is decoded only when the common `--schema` option was supplied;
ordinary text that happens to look like JSON is left as text.

Schema paths resolve relative to the caller's directory, independently of
`--cwd`. Due checks that the file contains a JSON object or boolean and captures
its contents once. Claude receives that JSON inline; Codex receives a private
temporary snapshot kept alive until execution settles. Missing files, invalid
JSON, and common/native schema-option conflicts fail before spawning.

The selected provider enforces its supported JSON Schema constraints. Due does
not implement an additional schema validator, convert between provider subsets,
or repair/retry responses. Missing Claude structured data or invalid Codex final
JSON after an otherwise successful schema run is a `protocol` error. Existing
provider, timeout, or cancellation failures retain their category and any
available response data. Default trace logs contain neither schemas nor responses.

`structured_output_present` distinguishes an absent answer (`false`) from an
explicit JSON null (`true` with `structured_output: null`).
`answer_schema_validation` is `not_requested` without the common `--schema`, or
`not_performed` with it, including dry runs and resolved failures. Intake
failures use `not_resolved` when Due cannot determine schema intent; an explicit
CLI `--schema` still yields `not_performed`. It describes independent validation
by Due, not the provider's enforcement or a review verdict. If answer validity
is a gate, the caller must run its own schema validator.

## Live progress

```sh
due run --provider claude -p "Fix the tests" --trace
due run --provider codex -p "Fix the tests" --trace --trace-file run.jsonl
due run --provider codex --stdin --output json --trace=json 2>progress.jsonl
```

Tracing reports process startup, session and turn activity, action starts and
completion or failure, message counts, elapsed time, and the final outcome. Quiet
periods produce a heartbeat every five seconds with the time since the last
activity. `--trace-interval 1s` changes that period. It is a liveness indicator,
not a percentage or a claim that the provider is making progress. Claude also
reports the start of thinking and text blocks without their contents.

Both adapters emit `activity_started` and `activity_completed` using the common
`activity` categories `command`, `file_change`, `mcp_call`, `web_search`,
`plan_update`, `status_update`, `tool_call`, and `unknown`. For example, Claude
`Bash` and Codex `command_execution` both become `command`. Completion has a
`status` of `succeeded`, `failed`, `cancelled`, or `unknown`; absent native status
is not invented. Starts or completion may be absent when the provider does not
report them. Activity and message counts describe observed events, not equivalent
units of work across providers. Assistant content is not counted as activity.
Codex `error` items are diagnostics, not activities: each completion produces a
`provider_error` record without its message, and the run outcome still comes
from the turn's terminal event.

### Trace records

Every JSONL record has `schema_version: 1`, a fresh UUID `run_id`, a monotonically
increasing `seq`, Unix `timestamp_ms`, `elapsed_ms`, `provider`, and `event`.
Available fields include native `session_id`, child `pid`/Unix `pgid`,
`activity_id`, tool names, `activity_duration_ms`, activity counters, message
byte counts, and `idle_ms`.
`run_finished` includes status, exit code, error kind, usage, and available cost.
Its `scope: "execution"` describes the normalized execution outcome before final
stdout delivery; a later stdout write failure can independently make Due exit 1.
The same records go to both sinks when file and JSON stderr tracing are enabled.
The final JSON result includes the matching `run_id` and a `trace` summary.

### Content limits

Default trace records exclude prompt text, argv, paths, model-generated text,
reasoning, tool arguments/results, and raw diagnostics. IDs and tool names are retained
with length/character restrictions. Due uses an explicit `tracing` target
allowlist; `RUST_LOG=trace` does not enable dependency payload logging. With
stderr tracing enabled (text or JSON), native diagnostics and Due's error-message
text are suppressed to keep that channel content-limited and JSONL parseable.
The trace retains the error kind and exit code. Use `--output json` for detailed
runtime errors on stdout; those errors and final responses can contain provider content.

Captured provider stdout and stderr each have a cumulative 64 MiB ceiling in
buffered and streaming runs. Exceeding either ceiling fails the run; it does not
return a shortened successful answer. Codex final-message files have a 4 MiB
read ceiling and must be regular files. Diagnostic and error text is limited to
16 KiB of UTF-8, including a visible truncation marker. These are byte limits
on captured streams and the final file, not per-event limits or a hard process
memory ceiling. On Unix (including macOS and Linux), final-message reads use
`O_NOFOLLOW` and `O_NONBLOCK`. No-follow behavior is not verified on non-Unix
platforms.

Use `--trace-detail` with `--trace` or `--trace-file` to include supported command,
file/tool labels and user-facing assistant messages. Details are bounded and
marked when truncated; text output escapes terminal controls. They may contain
paths, command arguments, or other provider content, so enable them deliberately.
Reasoning/thinking, prompts, and raw tool results are never detail sources.

### Trace files and loss

Trace files are created exclusively, with mode `0600` on Unix; existing files
are never overwritten. Relative paths resolve from the caller's directory.
File creation failures stop before launching a provider. Records use a bounded
queue so slow logging cannot block provider cleanup; overflow drops records.
Shutdown allows up to two seconds to drain the queue. The final JSON's
`trace.incomplete` reports dropped records, sink failure, or incomplete draining,
and `trace.dropped_records` counts queue drops. Sequence gaps also reveal loss.
An unresponsive sink may miss the terminal event; tracing never changes the
provider's exit status.

Tracing is opt-in and uses the wrappers' live event APIs. Final stdout remains a
single answer or result document. With only `--trace-file`, normal stderr
diagnostics remain enabled, except successful Codex stderr is unavailable through
the current wrapper streaming API. Dry runs log planning only and never spawn a
provider.

Codex buffered and traced execution parse only trim-start object-prefixed JSONL
lines as events. Other lines are ignored and are outside event/trace completeness
accounting. Both paths use UTF-8 replacement decoding for captured stdout/stderr;
malformed object-prefixed JSON still fails, including after a terminal success.
Accepted prefix text, session and usage are retained when available. Streaming
parser cleanup can stop the child before its natural exit, so that exit code may
be unknown even when buffered execution observes one.

If a Claude stream ends without a terminal result, JSON failure output retains
the latest complete text-bearing top-level assistant message, concatenating its
text blocks. Later tool-only messages do not erase it. Thinking, tool payloads,
nested messages and token deltas are excluded. Native subagent messages with a
non-null `parent_tool_use_id` are excluded from answer recovery, even though they
are top-level JSONL records. An absent or null marker identifies root-session
text; malformed non-null markers are excluded as well. Valid terminal results
remain authoritative, including empty failures and structured-only success. Missing
terminal output remains a failure; retained text is not native structured output
and is never printed as an answer in text-mode failures.
Fallback applies only when no result event was received. An invalid result
object still fails without substituting assistant text.

### Summarizing a saved trace

Inspect a saved trace without starting a provider:

```sh
due trace summarize run.jsonl
due trace summarize run.jsonl --output json
```

The summary groups records by run, reports observed execution outcomes and
activities, and surfaces sequence gaps or missing boundaries. It tolerates
unknown event kinds for forward compatibility. A missing terminal event leaves
the execution outcome unknown; it does not turn an incomplete log into a failed
agent run. The summary command's exit status describes reading and parsing the
log, not whether the recorded agent succeeded. Malformed or oversized records
fail instead of silently disappearing. Memory is bounded: at most 1 MiB per line,
128 runs, 256 event types per run, and 4,096 unmatched activity IDs per run; split
larger logs before summarizing. `due schema trace-summary` exports the summary
and error contract. A summary cannot detect every sink loss;
retain the final result's trace-delivery metadata too.

## Native options

The first `--` separates Due's options from the provider's. Remaining tokens go
directly into the query/exec/resume invocation, after generated options and before
native positionals. Due never evaluates a shell command string.

```sh
due run --provider claude --stdin -- --permission-mode acceptEdits --max-turns 5
due run --provider codex --stdin -- -c 'approval_policy="never"' -c 'sandbox_mode="workspace-write"'

# Provider-only effort values are allowed when the common --effort is absent.
due run --provider claude -p "Review this" -- --effort xhigh
due run --provider codex -p "Review this" -- -c 'model_reasoning_effort="xhigh"'
```

Due reserves prompt transport, output format/final-message destination, cwd,
session selection, persistence, help, and version controls. Use shared options for
those operations. Native model/effort options conflict only when their shared
equivalent is present in the merged task. The same rule applies to common `--schema`
versus Claude `--json-schema` or Codex `--output-schema`. Checks cover aliases,
attached short values, `--flag=value`, and Codex `-c`/`--config` overrides,
including quoted TOML keys.

Passthrough accepts options, not another prompt or command. A second `--`, a bare
leading operand, or an unexpected extra positional is rejected. Known native
options consume their normal values, including flag-looking values for a required
single-value option:

```sh
due run --provider claude -p "Summarize" -- --system-prompt '-respect input'
```

Unknown options may consume one ordinary separated value. Use `--new-option=value`
when that value starts with a hyphen; use repeated or attached options for new
multi-value syntax. Due cannot infer the arity of every future provider option,
so the native CLI owns remaining syntax and compatibility checks. Passthrough
validation is not a security boundary. Native settings files and profiles can
also affect execution. Permissions, tools, MCP configuration, schemas, and budget
controls remain provider-specific; Due does not claim equivalent policy behavior.

### Unattended Codex permissions

For unattended local work, combine a writable workspace sandbox with approval
policy `never`. This permits sandboxed edits and tests while rejecting requests
for additional approval. It is a workspace boundary, not Claude's exact-command
or individual-file allowlist. The prompt still defines the task's narrower scope;
independently verify which files changed.

The reusable [Codex workspace profile](../examples/codex-workspace-profile.json)
sets `sandbox_mode="workspace-write"`, `approval_policy="never"`,
`agents.enabled=false`, `features.multi_agent_v2=false`, and these
`sandbox_workspace_write` values:
`network_access=false`, `writable_roots=[]`, `exclude_slash_tmp=true`, and
`exclude_tmpdir_env_var=true`. All eight settings are native `-c` overrides. The explicit v2 disable closes
the precedence exception in the [versioned native control audit](native-control-audit.md).
`ephemeral: false` allows persistence; the profile contains no task-specific
cwd, prompt, session, answer schema, model, effort, deadline, or credentials.

This requests no additional writable roots or usual `/tmp` and `$TMPDIR` write
grants. Choose a trusted, authorized Git checkout as `--cwd`. If the task's tests
need temporary files, instruct those test commands to use a workspace-local
directory, for example `TMPDIR="$PWD/test-tmp" python3 -m unittest` from the task
cwd. Do not export workspace-local `TMPDIR` to Due itself: its temporary schema
snapshot and final-message file must stay outside every worker writable root.
Other writable locations require explicit authorization.
Workspace-write does not restrict all reads to that directory. Network restrictions
here apply to sandboxed commands; the provider's model connection and connected
tools have separate controls. Inherited native execution rules and managed policy
still apply. Disabling built-in agents does not prevent every subprocess launch.
Do not add bypass flags or enable network access just to make a denied check pass.
See the [native sandbox guide](https://learn.chatgpt.com/docs/sandboxing) and
[configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference).

Due forwards these settings; it does not independently enforce or validate the
sandbox. A dry run checks the merged request and Due's reserved-option conflicts,
not native CLI capability or the host's effective policy. In particular, a dry
run can succeed with `--sandbox workspace-write` even though native
`codex exec resume` rejects that option. Due does not strip incompatible flags
or weaken permissions to make a run pass.
Verify each machine with a disposable task and harmless negative controls:
observe an actual denied write outside the writable roots, then independently
check the target is unchanged. A worker declining to attempt the write does not
by itself demonstrate sandbox enforcement. Record denied checks separately from
execution status: the agent can finish successfully after an expected denial.

In the profile acceptance below, Due's trace omitted the denied write as a failed
activity (known issue #6). The caller matched the actual `PermissionError` in
native session tool output and checked unchanged canaries. This batch did not
inspect raw native `exec --json` before parsing, so it does not establish where
the event was lost. Zero failed activities or zero dropped trace records does
not prove that no permissions were denied. A later [correlated capture](native-trace-evidence.md)
reproduced one inline-command omission in native stdout before Due parsing,
while separate script controls emitted failures correctly.

### Reusable Codex workspace profile

Use the same [version 1 profile](../examples/codex-workspace-profile.json) for
both commands below. Codex CLI **0.162.0** exposes `-c/--config` on `exec` and
`exec resume`, but `--sandbox` only on `exec`.

Caller live acceptance with installed Due **0.1.0**, Codex **0.162.0**, and
**macOS 27 arm64** used the earlier seven-setting profile for two same-workspace turns
with new prompts and separate results/traces, supplying the answer schema per turn. The native session ID stayed
the same, and the follow-up recalled a token absent from its prompt. Receipt edits
and unit tests passed in both turns. One outside write per turn raised a native
`PermissionError`; both canaries remained unchanged. The 18/19-record traces were
contiguous and complete with no reported drops, subject to the denial gap above.
Native contexts recorded workspace-write, approval `never`, networking disabled,
and both excluded temporary-directory grants true. All seven original requested settings
were forwarded, but the native record did not expose `agents.enabled` or explicit
`writable_roots`: their effective values were not independently confirmed.
No delegation was observed; that does not confirm effective agent disabling.
The profile now additionally disables `features.multi_agent_v2`, which takes
precedence over `agents.enabled` in published Codex 0.162.0 source. The revised
eight-setting profile was accepted in fresh/resumed 0.162.0 trace probes with
workspace writes and denied outside controls; this does not establish a runtime
tool inventory. See [the control audit](native-control-audit.md) and
[the correlated trace evidence](native-trace-evidence.md).

This establishes CLI acceptance and observed effects on that version and host,
not a security guarantee for every system. Confirm settings in native effective
context where available and report unsupported or unexposed settings rather than
silently removing them. Automated fake-provider tests establish only Due's routing
and result/trace contracts; actual history continuity was checked separately above.

The following shell recipe uses `jq`. Replace the paths with an existing
authorized Git checkout and protected caller-owned copies of this profile and
answer schema, outside every worker writable root. Prepare `first-task.txt` and
`follow-up.txt` with separate bounded assignments in the caller-owned
`input_root`, outside every worker writable root.
Keep both turns in that same workspace; cross-workspace resume has not been
established. Paths in a request profile resolve from the caller's directory,
not the profile directory. Absolute paths here avoid that ambiguity. Codex's
normal Git-repository/trust check remains enabled; initialize an authorized
disposable Git repository first when using a new smoke workspace. The Git
preflight below does not override native trust rules. Do not add
`--skip-git-repo-check` automatically. Any authorized new native option belongs
in a copy of the profile's **complete** `native_args` list: passing it alone
after `--` replaces all eight settings.

Use a canonical workspace path (`pwd -P`) for native grants. An outer host that
rejects symlinked writable roots before process creation needs a supported host
workspace/permission correction; changing this profile cannot fix that launch
failure. See [troubleshooting](troubleshooting.md).

Choose an existing, caller-writable `artifact_root` outside every worker writable
root. Each Due invocation below sets its own `TMPDIR` to the protected artifact
directory, keeping schema snapshots and final-message files outside those roots.
This per-command environment value does not widen worker grants: the profile
excludes the `TMPDIR` write grant. Keep caller-owned profiles, schemas and prompts
outside worker writable roots too. Set test-command temporary paths inside the
workspace separately; do not export that path to Due. Worker commands inherit
the protected `TMPDIR` and cannot write there, so set a workspace-local `TMPDIR`
on any command that needs temporary files, including tests.

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
and effects. Use observed checks and artifacts; a native session record is
optional extra evidence when accessible. Report missing evidence or denied
checks without claiming they passed. These are caller-owned checks, which can
be automated within existing authorization; no human approval or manual ID paste
is required. Due does not independently validate the answer or effective policy.

After those checks, the next block selects the ID from this fresh result only,
checks its run ID against every trace record, and matches all emitted session
IDs. This trace-enabled recipe requires a `session_started` marker and completed
trace; if either is missing, investigate before follow-up. It then sets
`verified_session_id` programmatically. Due neither selects the last session
nor manages a separate session database.

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

The profile stays unchanged and no arguments after `--` replace its grants.
Each invocation supplies its own prompt, schema, cwd, result, and fresh trace
path; the second adds only an explicit native resume ID. Save each execution
status before running `due trace summarize` or an independent answer validator,
which have their own exit codes. Require `structured_output_present`, validate
`structured_output` against the supplied schema, and inspect effects separately:
`answer_schema_validation: "not_performed"` is not a validation pass. Inspect
resumed native cwd/permissions as well; successful execution does not prove a
denied check ran or passed. Report blocked checks as blocked and preserve the
policy, including inherited denials.

### Unattended Claude permissions

`--tools` controls which built-in tools Claude can call; it does not approve their use.
`acceptEdits` permits file edits, but shell commands such as tests still need an
allow rule. Due forwards these native settings and does not answer approval
requests.

For a bounded repair to `receipt.py` with Python unit tests, use a partial profile:

```json
{
  "schema_version": 1,
  "provider": "claude",
  "ephemeral": true,
  "native_args": [
    "--permission-mode", "dontAsk",
    "--permission-prompts", "none",
    "--tools", "Read,Edit,Bash",
    "--allowedTools", "Edit(/receipt.py)", "Bash(python3 -B -m unittest -v)"
  ]
}
```

Save as `claude-profile.json`, then supply the repository and assignment:

```sh
due run --request claude-profile.json --cwd /path/to/repo --stdin \
  --output json --trace --trace-file run.jsonl < task.txt > result.json
```

Adapt the file and command grants to the authorized task. Tell the worker to run
that exact test command from the supplied cwd, without wrappers or redirects.
In CLI permission rules, `/receipt.py` is anchored to Claude's starting directory
(Due's `--cwd`), not the filesystem root. Quote each rule when passing it through
the shell. Arguments after `--` replace a profile's **whole** native argument list.

`dontAsk` denies actions that would need approval; ordinary permitted reads and
inherited allows still apply. Matching deny and ask rules are evaluated against
the input Claude checks. A PreToolUse hook can rewrite that input before
matching, so test any required block with the actual worker settings and a
harmless command. A Due dry run previews arguments only. See
`due skills show run` for the permission probe and denial evidence.
`--permission-prompts none` also tells Claude nobody can answer prompts or approve
retries; it requires Claude Code 2.1.259 or later. Check the installed native CLI
before reusing a profile on another machine. See Claude's
[permission rules](https://code.claude.com/docs/en/permissions) and
[unattended-run guidance](https://code.claude.com/docs/en/headless#turn-off-permission-prompts-in-unattended-runs).

These grants supplement native policy; they are not an isolated sandbox. An
approved test command executes repository code with the process's existing
filesystem/network restrictions. Only grant commands appropriate to that task.
No global settings changes or permission bypass flags are needed.

Inspect the result envelope and actual effects after the run. A successful Due
execution can still contain blocked tests: require the expected checks to have
run, validate their reported result, and independently inspect the diff and tests.
If permission is denied, report the partial outcome instead of changing policy
or retrying automatically. A dry run validates Due's invocation, not Claude's
permission decisions.

## Results and errors

Text mode prints only the successful final answer, with a terminal newline.
Diagnostics go to stderr. Failed runs do not print partial answers in text mode.
For successful Codex turns, text comes from the `--output-last-message` file,
excluding intermediate commentary. Claude text comes from the terminal result,
including when verbose mode wraps records in an array. An empty successful result
with `structured_output` returns the serialized structured response.

For buffered Claude invocations, Due requests native `--output-format json --verbose`
and reads the resulting array of records. In a caller capture with
Claude Code 2.1.312, a background `sleep 8` produced two root `result` records
with a second `system/init` and `assistant` record between them; nonverbose JSON
contained only the final result. Due uses the last root result for answer,
status, structured output, session, and cumulative cost. It adds reported usage
counters across root result segments; a missing segment counter stays unknown.
Traced JSONL uses the same accounting and emits one completion phase for the
valid final native result when the invocation ends. Child results do not contribute.
Only a native JSON array or a single result object is supported in buffered
mode; concatenated JSON documents are not a supported transport.
Verbose buffered output captures the full native transcript, including tool
inputs and results, subject to the existing 64 MiB stdout capture limit.

Failed JSON results may retain partial output: Codex preserves a readable
final-message file or observed assistant text, including intermediate commentary;
with `--schema`, retained Codex JSON may also be decoded as `structured_output`.
A traced Claude run with no terminal result event preserves the latest complete
root-session assistant message with nonempty text. Valid terminal results remain
authoritative; malformed terminal events fail without substituting assistant text.
Availability depends on the failure path, provider and wrapper. Neither partial
text nor a locally valid partial answer is a final answer or successful execution.
Retained text does not change the failure status. See [live progress](#live-progress)
for parsing details.

JSON mode prints one document for a successful run or any error after argument
parsing. Clap syntax/help/version output follows ordinary CLI conventions;
argument syntax errors go to stderr with exit code 2.

```json
{
  "schema_version": 1,
  "provider": "codex",
  "status": "succeeded",
  "text": "The final answer",
  "provenance": {
    "requested": {"model": null, "effort": null},
    "reported": {"models": [], "effort": null, "truncated": false}
  },
  "structured_output": null,
  "structured_output_present": false,
  "answer_schema_validation": "not_requested",
  "session_id": "native-thread-id",
  "usage": null,
  "cost_usd": null,
  "duration_ms": 1250,
  "error": null
}
```

`due schema result` exports the result JSON Schema; `due schema trace` exports
the JSONL record schema. All exports are embedded, offline, and match the installed
CLI version. `schema_version` versions each data contract.

Additive result fields may appear within v1. Consumers should ignore unfamiliar
fields and validate against the producing binary's exported schema. Older strict
schemas can still reject newer fields through `additionalProperties`. Current
results always emit `provenance`; the exported v1 schema makes it optional so
older saved records remain valid.

`provenance.requested` records merged common `model` and `effort` values, including
plans and resolved failures. Omitted values stay null; rejected intake uses
`requested: null`. `reported.models` contains unique IDs from Claude assistant
message metadata observed during the turn, including streaming message starts
and child messages. The reserved `<synthetic>` placeholder, requested aliases,
assistant prose, configuration/init metadata and accounting such as `modelUsage`
do not establish a reported model for the turn. Codex's pinned turn surfaces
supply no actual model report, so its list stays empty. Actual effort is unknown
for both providers and remains null. The list is capped at 16 IDs of 256 UTF-8
bytes each; `truncated` marks omitted IDs at either bound. Empty means unknown,
and multiple IDs do not select one effective model. RPC and JavaScript turn
results use the same field; observations survive failures and reset each turn.

`status` is `succeeded`, `failed`, `timed_out`, `cancelled`, or `planned`. Dry runs
add `invocation` with `binary`, `cwd`, `args`, and `stdin_bytes`. Errors contain
`kind`, `message`, and nullable `provider_exit_code`. Runtime failures retain
available session, cost, and partial-text data; availability depends on what the
provider and wrapper returned. Request intake failures have `provider: null`;
results after provider resolution identify `claude` or `codex`.

Error kinds are `invalid_input`, `authentication`, `max_turns`, `max_budget`,
`configuration`, `session_not_found`, `unsupported`, `provider`, `protocol`, `io`,
`timeout`, `cancelled`, and `not_found`. Typed wrapper failures and verified
native error codes supply these categories; unrecognized native failures remain
`provider`. No message parsing is added by Due. `unsupported` covers a reported
unsupported CLI version. Provider error categories still exit 1; callers can
inspect `error.kind` for recovery decisions without parsing prose. Partial
metadata is retained whenever the wrapper makes it available; some classified
buffered Claude errors expose only session and cost.

Usage fields are nullable native counters: `input_tokens`, `cached_input_tokens`,
`cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens`, and
`total_tokens`. Cache inclusion in input counts differs between providers; Due
preserves the numbers and does not invent totals or dollar estimates. Codex cost
is always null. `duration_ms` is Due's elapsed time, including input/setup/cleanup.

| Exit code | Meaning |
| --- | --- |
| `0` | Successful turn or dry run |
| `1` | Provider, protocol, or I/O failure |
| `2` | Invalid invocation, input, or conflicting options |
| `124` | Execution timeout |
| `127` | Provider executable not found |
| `130` | Cancelled by Ctrl-C or Unix SIGTERM/SIGHUP |

## Scope and tests

For blocked launches, missing binaries, native login/state access, task denials,
Git/resume mismatches or schema rejection, use [troubleshooting](troubleshooting.md).
Preserve failed results, traces and partial edits before a deliberate follow-up.

The CI checks are:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

Tests launch fake provider executables, not models. They exercise actual argv,
stdin, cwd, native passthrough, resume/ephemeral mapping, dry-run, final-answer
normalization, failures, and Unix timeout/SIGINT/SIGTERM/SIGHUP descendant cleanup. Tracing tests
also hold a gated fake provider mid-run to verify early event delivery, heartbeat
behavior, content omission, sink handling, and cleanup with a blocked trace
consumer. Protocol fixtures follow the wrappers' recorded CLI shapes. These
`due run` paths remain one noninteractive turn. Separate RPC and harness tests
cover resident sessions, streamed items, async supervision and process settlement.

The implementation uses Clap, one shared request/result contract, and separate
provider adapters over the existing wrappers' cancellable execution APIs. The
`due run` scope is one invocation, one provider run, one result, and optional observation.
Shared features translate provider differences or improve execution reliability;
agent lifetimes, queues, automatic retries, context catalogs, configuration
hierarchies, and repository management belong to the caller.

The plugin and embedded skills select the installed one-shot or supervised
interface. `due script` adds a foreground JavaScript workflow above resident RPC;
it does not add an MCP server, background daemon or persistent queue. A run request
still describes one execution. Artifacts and external effects still need the caller's independent
verification; Due does not collect verified effect receipts.
