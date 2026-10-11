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

The MVP pins the passthrough additions in
[claude-wrapper #817](https://github.com/joshrotenberg/claude-wrapper/pull/817) and
[codex-wrapper #148](https://github.com/joshrotenberg/codex-wrapper/pull/148) by full
Git commit. The first build needs GitHub and crates.io access. It does not depend
on local wrapper checkouts. Move to released crate versions once those additions
are published; keep `Cargo.lock` committed for reproducible application builds.

## Shared options

| Option | Behavior |
| --- | --- |
| `--request FILE\|-` | Read one versioned JSON request from a file or stdin, instead of execution flags. |
| `--provider claude\|codex` | Required with execution flags; selects the native CLI. |
| `-p, --prompt TEXT` | Prompt text. Without `--request`, exactly one of this and `--stdin` is required. |
| `--stdin` | Read the entire UTF-8 prompt from stdin. Empty prompts are rejected. |
| `--model ID` | Native model name/alias. Omit to keep the provider default. |
| `--effort low\|medium\|high` | Native effort level. Support and computation vary by provider/model. |
| `-C, --cwd DIR` | Child working directory, resolved from the caller's cwd. |
| `--timeout 30s` | Execution deadline; supports durations such as `5m`. No default. |
| `--output text\|json` | Final answer text by default, or one versioned JSON result. |
| `--schema FILE` | Request a structured answer using a JSON Schema; preserve it as a JSON value in the result. |
| `--resume ID` | Continue a native session/thread with the supplied prompt. |
| `--ephemeral` | Disable native session persistence. Conflicts with `--resume`. |
| `--dry-run` | Show executable, argv, cwd, and stdin size without launching a provider. |
| `--binary PATH` | Override the selected provider's executable. |
| `--trace[=text\|json]` | Live progress on stderr; bare `--trace` uses readable text. |
| `--trace-file PATH` | Create a new JSONL progress log; can be used with or without stderr tracing. |
| `--trace-interval 5s` | Heartbeat period while tracing, after reading the prompt. Must be positive. |

Prompts always travel to the child through stdin. A prompt supplied on Due's own
command line is still visible in Due's argv; use `--stdin` when that matters.
Dry runs omit prompt contents and work even when the provider is not installed.
Provider options themselves are shown in dry-run output, including an inline
Claude schema, so secrets in native options or schemas are not redacted.

Relative executable paths and PATH lookups resolve before `--cwd` changes the
child's directory. The execution timeout starts when the provider runs, after the
complete prompt has been read. Shutdown waits for wrapper cleanup and may extend
beyond the deadline. Ctrl-C similarly awaits cleanup before returning. On Unix,
the wrappers terminate the owned process group; elsewhere their process cleanup
capabilities apply. Due performs no automatic retries.

Resume IDs stay provider-specific. `session_id` in Due's result is Claude's native
session ID or Codex's native thread ID. Ephemeral runs may report an ID that cannot
be resumed. There is no local session database or implicit "latest" selection.

## JSON requests

Agent callers can supply one JSON object instead of assembling execution flags:

```sh
due run --request examples/run-request.json --output json
cat examples/run-request.json | due run --request - --output json --trace
due run --request examples/run-request.json --dry-run --output json
```

The [request schema](schemas/run-request.schema.json) describes the input shape;
the [example](examples/run-request.json) requests a read-only repository summary.
The required fields are `schema_version: 1`, `provider` (`claude` or `codex`),
and a literal, non-whitespace `prompt` string. Optional execution fields are:

| Field | Accepted value or default |
| --- | --- |
| `model` | Nonempty native model string; omitted or null keeps the provider default. |
| `effort` | `low`, `medium`, `high`, or null. |
| `cwd` | Working directory string or null; defaults to the caller's directory. |
| `timeout` | Positive duration string such as `30s` or `5m`, or null for no deadline. |
| `schema` | Answer-schema filename string or null. |
| `resume` | Nonempty native session ID string or null. |
| `ephemeral` | Boolean, default false; true conflicts with a non-null `resume`. |
| `native_args` | Ordered array of strings, default empty, using the same passthrough rules. |

Unknown or duplicate keys, unsupported versions, and wrong types are rejected.
`ephemeral` and `native_args` cannot be null. All fields are top-level; there are
no nested option groups or recursive prompt/request inputs. `--request -` consumes
the whole stdin stream as JSON; the prompt comes from that document's `prompt`.
Request `cwd` and `schema` paths resolve from the caller's directory, not the
request file's directory. Paths inside `native_args` follow the native CLI's rules.
`schema` names the model-answer schema; it is distinct from the request schema.

Do not mix `--request` with provider, prompt, stdin, model, effort, cwd, timeout,
schema, resume, ephemeral, or trailing native-argument flags. Only `--output`,
`--dry-run`, `--binary`, `--trace`, `--trace-file`, and `--trace-interval` accompany
a request; they are local CLI controls and are not request fields. Output still
defaults to text. The machine schema documents structural constraints; Due also
checks duplicate keys, parses durations, and requires a positive timeout at intake.
The schema intentionally does not approximate the full duration grammar with a regex.

Request intake errors with `--output json` use the existing result envelope with
`provider: null`, `error.kind: invalid_input`, and exit 2. After resolution, runtime
errors identify the selected provider. Clap syntax errors use stderr and exit 2.
Request intake precedes tracing: a failure may print a plain stderr diagnostic
even with `--trace=json`, and no trace file is created.
A request describes one run; it does not collect or verify side effects.

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
available response data. Trace logs contain neither schemas nor responses.

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

Trace records exclude prompt text, argv, paths, model-generated text, reasoning,
tool arguments/results, and raw diagnostics. IDs and tool names are retained
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
equivalent was explicitly supplied. The same rule applies to common `--schema`
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
  "session_id": "native-thread-id",
  "usage": null,
  "cost_usd": null,
  "duration_ms": 1250,
  "error": null
}
```

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
