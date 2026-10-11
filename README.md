# due

One command-line interface for Claude Code and Codex. Run a single turn with a
common set of options, then pass provider-specific options after `--`.
Each CLI invocation owns one foreground provider run; the caller and operating
system own concurrency, scheduling, and any later retry.

```sh
due run --provider claude -p "Explain this repository" --effort high
due run --provider codex -p "Explain this repository" --effort high
cat task.txt | due run --provider codex --stdin --output json
due run --request examples/run-request.json --output json
due run --request examples/profile.json --stdin --output json < task.txt

due run --provider claude -p "Fix the tests" -- --max-turns 5
due run --provider codex -p "Fix the tests" -- --sandbox workspace-write
```

## Agent entry point and embedded skills

The skills-only plugin in [`plugin/due`](plugin/due) provides one short
[`delegate` skill](plugin/due/skills/delegate/SKILL.md). It starts an agent with
the installed binary's help and skill catalog. The plugin requires local command
execution, Due on PATH, and the selected provider CLI installed and authenticated.
It follows the [Agent Plugins package format](https://agent-plugins.org/plugin-authors/build-an-agent-plugin).
Load that directory through a compatible client's local plugin installation flow.
The package does not install a provider or change the caller's authorization.

Detailed guidance ships inside the executable, so it stays available alongside
the installed behavior without a source checkout, provider process, or network:

```sh
due --help
due run --help
due skills                         # Names and short descriptions
due skills list --output json      # Versioned catalog for automated selection
due skills show run                # Scope, execution, and verifying effects
due skills show contracts          # Structured answers and result semantics
due skills show observe            # Progress, trace loss, and cancellation
due skills show contracts --output json
due schema request                # Installed input contract as JSON Schema
due schema result                 # Installed result envelope contract
due schema trace                  # Installed progress record contract
due schema trace-summary          # Installed offline summary contract
due trace summarize run.jsonl --output json
```

`skills show` prints a complete Agent Skills `SKILL.md` by default. JSON catalog
and document output include `schema_version: 1` and `cli_version`; unknown names
return a structured `invalid_input` error and exit 2 in JSON mode. Normal argument
syntax errors use stderr and exit 2. These commands are read-only and do not need
a provider installation. The canonical instructions live in `embedded-skills/`
and are included at compile time; changing them requires rebuilding the binary.

The entry skill stays small; load only the detailed guide relevant to the task.
The current contract concerns execution and returned data. Artifact and external
side effects still require the caller's independent verification.

## Build and install

Requires Rust 1.90 or newer, and whichever provider CLI you intend to use,
installed and authenticated. Due inherits that CLI's environment, settings, and
permission defaults. It does not install providers or manage credentials.

From this directory:

```sh
cargo build --release --locked
./target/release/due run --help

# Optional: install the binary on your Cargo bin path.
cargo install --path . --locked
```

The MVP pins full Git commits containing passthrough support
([claude-wrapper #817](https://github.com/joshrotenberg/claude-wrapper/pull/817),
[codex-wrapper #148](https://github.com/joshrotenberg/codex-wrapper/pull/148)) and
cooperative shutdown
([claude-wrapper #818](https://github.com/joshrotenberg/claude-wrapper/pull/818),
[codex-wrapper #149](https://github.com/joshrotenberg/codex-wrapper/pull/149)).
The first build needs GitHub and crates.io access. It does not depend on local
wrapper checkouts. Keep the Git pins while the wrappers are exercised through
Due; releases can follow once their behavior is settled. Keep `Cargo.lock`
committed for reproducible application builds.

## Shared options

| Option | Behavior |
| --- | --- |
| `--request FILE\|-` | Read a versioned JSON request or partial profile; explicit CLI values override it. |
| `--provider claude\|codex` | Select the native CLI; required after merging request and flags. |
| `-p, --prompt TEXT` | Prompt text, replacing any profile prompt; conflicts with `--stdin`. |
| `--stdin` | Read the entire UTF-8 prompt, replacing any profile prompt; conflicts with `--request -`. |
| `--model ID` | Native model name/alias. Omit to keep the provider default. |
| `--effort low\|medium\|high` | Native effort level. Support and computation vary by provider/model. |
| `-C, --cwd DIR` | Child working directory, resolved from the caller's cwd. |
| `--timeout 30s` | Execution deadline; supports durations such as `5m`. No default. |
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

Prompts always travel to the child through stdin. A prompt supplied on Due's own
command line is still visible in Due's argv; use `--stdin` when that matters.
Dry runs validate input shape, the merged task, native-option conflicts, working
directory, and answer-schema JSON. They omit prompt contents and work even when
the provider is not installed. They do not check authentication, model support,
native option compatibility, or provider-specific schema constraints. A dry run
may create an explicitly requested trace file and a temporary schema snapshot.
Provider options themselves are shown in dry-run output, including an inline
Claude schema, so secrets in native options or schemas are not redacted.

Relative executable paths and PATH lookups resolve before `--cwd` changes the
child's directory. The execution timeout starts when the provider runs, after the
complete prompt has been read. On Unix, Ctrl-C and timeouts ask the native CLI to
interrupt with SIGINT, while the wrappers drain its output for a two-second grace
period. They then force-stop the remaining owned process group and reap the direct
child before Due returns. Shutdown may extend beyond the execution deadline.
Native interruption lets the CLI clean up tool processes in separate groups;
forced group termination alone cannot reach those processes. Cleanup of escaped
processes still depends on native cooperation. Other platforms use the wrappers'
available process cleanup. Due performs no automatic retries.

Resume IDs stay provider-specific. `session_id` in Due's result is Claude's native
session ID or Codex's native thread ID. Ephemeral runs may report an ID that cannot
be resumed. There is no local session database or implicit "latest" selection.

## JSON requests and profiles

A request file supplies defaults for one invocation. Explicit execution flags
replace its values; omitted flags retain them. This supports both complete tasks
and reusable partial profiles:

```json
{"schema_version":1,"provider":"codex","timeout":"5m","ephemeral":true}
```

```sh
due run --request examples/profile.json --stdin --output json < task.txt
due run --request examples/profile.json --provider claude -p "Review the diff" \
  --timeout 2m --dry-run --output json
due run --request task.json --no-ephemeral --resume native-session-id --output json
cat task.json | due run --request - -p "Use this replacement prompt" --output json
```

Only `schema_version: 1` is required in the file. A provider and non-whitespace
prompt must be present after merging, or use `--stdin` for the prompt.
The [request schema](schemas/run-request.schema.json) describes the profile's
structural shape; `due schema request` exports the installed version offline.
The [full request](examples/run-request.json) and [profile](examples/profile.json)
are examples. There is no profile search path, inheritance chain, or config store.

| Field | Accepted final value or default |
| --- | --- |
| `provider` | `claude` or `codex`; may be omitted but cannot be null. |
| `prompt` | Non-whitespace literal string; may be omitted but cannot be null. |
| `model` | Nonempty native model string; omitted or null keeps the provider default. |
| `effort` | `low`, `medium`, `high`, or null. |
| `cwd` | Working directory string or null; defaults to the caller's directory. |
| `timeout` | Positive duration string such as `30s` or `5m`, or null for no deadline. |
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
the list. Switching providers keeps other inherited values, which must remain
valid for the selected provider; Due does not silently discard options.

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
`not_performed` with it, including dry runs and resolved failures. Intake failures use `not_resolved` when
Due cannot determine schema intent; an explicit CLI `--schema` still yields
`not_performed`. It describes independent
validation by Due, not the provider's enforcement or a review verdict. If answer
validity is a gate, the caller must run its own schema validator.

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

Default trace records exclude prompt text, argv, paths, model-generated text,
reasoning, tool arguments/results, and raw diagnostics. IDs and tool names are retained
with length/character restrictions. Due uses an explicit `tracing` target
allowlist; `RUST_LOG=trace` does not enable dependency payload logging. With
stderr tracing enabled, native diagnostics are suppressed to keep that channel
safe and JSONL parseable. Use `--output json` for detailed runtime errors on
stdout; those errors and final responses can contain provider content.

Trace files are created exclusively, with mode `0600` on Unix; existing files
are never overwritten. Relative paths resolve from the caller's directory.
File creation failures stop before launching a provider. Records use a bounded
queue so slow logging cannot block provider cleanup; overflow drops records.
Shutdown allows up to two seconds to drain the queue. The final JSON's
`trace.incomplete` reports dropped records, sink failure, or incomplete draining,
and `trace.dropped_records` counts queue drops. Sequence gaps also reveal loss.
An unresponsive sink may miss the terminal event; tracing never changes the
provider's exit status.

Use `--trace-detail` with `--trace` or `--trace-file` to include supported command,
file/tool labels and user-facing assistant messages. Details are bounded and
marked when truncated; text output escapes terminal controls. They may contain
paths, command arguments, or other provider content, so enable them deliberately.
Reasoning/thinking, prompts, and raw tool results are never detail sources.

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

Tracing is opt-in and uses the wrappers' live event APIs. Final stdout remains a
single answer or result document. With only `--trace-file`, normal stderr
diagnostics remain enabled, except successful Codex stderr is unavailable through
the current wrapper streaming API. Dry runs log planning only and never spawn a
provider.

## Native options

The first `--` separates Due's options from the provider's. Remaining tokens go
directly into the query/exec/resume invocation, after generated options and before
native positionals. Due never evaluates a shell command string.

```sh
due run --provider claude --stdin -- --permission-mode acceptEdits --max-turns 5
due run --provider codex --stdin -- -c 'approval_policy="never"' --sandbox workspace-write

# Provider-only effort values are allowed when the common --effort is absent.
due run --provider claude -p "Review this" -- --effort xhigh
due run --provider codex -p "Review this" -- -c 'model_reasoning_effort="xhigh"'
```

Due reserves prompt transport, output format/final-message destination, cwd,
session selection, persistence, help, and version controls. Use shared options for
those operations. Native model/effort options conflict only when their shared
equivalent is present in the merged task. The same rule applies to common `--schema`
versus Claude `--json-schema` or Codex `--output-schema`. Checks cover aliases, attached short values,
`--flag=value`, and Codex `-c`/`--config` overrides, including quoted TOML keys.

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

## Results and errors

Text mode prints only the successful final answer, with a terminal newline.
Diagnostics go to stderr. Failed runs do not print partial answers in text mode.
Codex text comes from its `--output-last-message` file, excluding intermediate
commentary. Claude text comes from the terminal result, including when verbose
mode wraps records in an array. An empty result with `structured_output` returns
the serialized structured response.

JSON mode prints one document for a successful run or any error after argument
parsing. Clap syntax/help/version output follows ordinary CLI conventions;
argument syntax errors go to stderr with exit code 2.

```json
{
  "schema_version": 1,
  "provider": "codex",
  "status": "succeeded",
  "text": "The final answer",
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
| `130` | Cancelled by Ctrl-C |

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Tests launch fake provider executables, not models. They exercise actual argv,
stdin, cwd, native passthrough, resume/ephemeral mapping, dry-run, final-answer
normalization, failures, and Unix timeout/Ctrl-C descendant cleanup. Tracing tests
also gate a live provider to verify early event delivery, heartbeat behavior,
content omission, sink handling, and cleanup with a blocked trace consumer.
Protocol fixtures follow the wrappers' recorded CLI shapes. The unit of execution
remains one noninteractive turn; no interactive UI, response streaming, scheduler, or
provider authentication flow is included.

The implementation uses Clap, one shared request/result contract, and separate
provider adapters over the existing wrappers' cancellable execution APIs. Its
scope is one invocation, one provider run, one result, and optional observation.
Shared features translate provider differences or improve execution reliability;
agent lifetimes, queues, automatic retries, context catalogs, configuration
hierarchies, and repository management belong to the caller.

The CLI, plugin, and embedded skills use the same foreground operation; they add
no MCP server, queue, scheduler, or retained agent runtime. Request input describes
one execution. Artifacts and external effects still need the caller's independent
verification; Due does not collect verified effect receipts.
