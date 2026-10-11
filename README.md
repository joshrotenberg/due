# due

One command-line interface for Claude Code and Codex. Run a single turn with a
common set of options, then pass provider-specific options after `--`.

```sh
due run --provider claude -p "Explain this repository" --effort high
due run --provider codex -p "Explain this repository" --effort high
cat task.txt | due run --provider codex --stdin --output json

due run --provider claude -p "Fix the tests" -- --max-turns 5
due run --provider codex -p "Fix the tests" -- --sandbox workspace-write
```

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
| `--provider claude\|codex` | Required; selects the native CLI. |
| `-p, --prompt TEXT` | Prompt text. Exactly one of this and `--stdin` is required. |
| `--stdin` | Read the entire UTF-8 prompt from stdin. Empty prompts are rejected. |
| `--model ID` | Native model name/alias. Omit to keep the provider default. |
| `--effort low\|medium\|high` | Native effort level. Support and computation vary by provider/model. |
| `-C, --cwd DIR` | Child working directory, resolved from the caller's cwd. |
| `--timeout 30s` | Execution deadline; supports durations such as `5m`. No default. |
| `--output text\|json` | Final answer text by default, or one versioned JSON result. |
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
Provider options themselves are shown in dry-run output, so secrets in native
options are not redacted.

Relative executable paths and PATH lookups resolve before `--cwd` changes the
child's directory. The execution timeout starts when the provider runs, after the
complete prompt has been read. Shutdown waits for wrapper cleanup and may extend
beyond the deadline. Ctrl-C similarly awaits cleanup before returning. On Unix,
the wrappers terminate the owned process group; elsewhere their process cleanup
capabilities apply. Due performs no automatic retries.

Resume IDs stay provider-specific. `session_id` in Due's result is Claude's native
session ID or Codex's native thread ID. Ephemeral runs may report an ID that cannot
be resumed. There is no local session database or implicit "latest" selection.

## Live progress

```sh
due run --provider claude -p "Fix the tests" --trace
due run --provider codex -p "Fix the tests" --trace --trace-file run.jsonl
due run --provider codex --stdin --output json --trace=json 2>progress.jsonl
```

Tracing reports process startup, session and turn activity, tool starts and
completion or failure, message counts, elapsed time, and the final outcome. Quiet
periods produce a heartbeat every five seconds with the time since the last
activity. `--trace-interval 1s` changes that period. It is a liveness indicator,
not a percentage or a claim that the provider is making progress. Claude also
reports the start of thinking and text blocks without their contents. Codex
identifies command execution as tool activity; other item kinds have generic
lifecycle events.

Every JSONL record has `schema_version: 1`, a fresh UUID `run_id`, a monotonically
increasing `seq`, Unix `timestamp_ms`, `elapsed_ms`, `provider`, and `event`.
Available fields include native `session_id`, child `pid`/Unix `pgid`, item IDs,
tool names, tool durations and counters, message byte counts, and `idle_ms`.
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
equivalent was explicitly supplied. Checks cover aliases, attached short values,
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
provider and wrapper returned.

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
provider adapters over the existing wrappers' cancellable execution APIs.

Next planned extension: a thin [agent plugin](https://agent-plugins.org) exposing
Due's single-run operation for subagent delegation.
