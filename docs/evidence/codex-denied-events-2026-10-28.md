# Codex denied-command events: recheck on Codex CLI 0.163.0

Snapshot: 2026-10-28. Tracks [#6](https://github.com/joshrotenberg/due/issues/6).
Evidence only; no Due runtime, wrapper pin, installed CLI or permission change.

## Result

On Codex CLI 0.163.0 the omission still occurs and is present in native output
before Due's parser. In five bounded turns through installed Due, a failed
command with no `command_execution` item in raw native stdout also had no
`CommandExecution` item in Codex's own session item records, while the session's
raw tool output recorded the command and its nonzero exit. Due's trace matched
what native stdout contained in every turn. No normalization defect was found.

The omitted commands were the ones whose output contained "Operation not
permitted". A command that only printed that text and exited 1, with no actual
denial, was also omitted. A neutral exit-1 failure and an actual denial reported
without that text were both emitted. This is consistent with an output-keyword
heuristic in the published 0.163.0 source (see [Source note](#source-note)), but
it is a bounded discrimination, not a confirmed internal root cause.

## Dependency on #87

The Codex profile and the prior evidence exist only in unmerged
[#87](https://github.com/joshrotenberg/due/pull/87). The profile was used
unchanged from its exact head
[`437aea5`](https://github.com/joshrotenberg/due/blob/437aea507f4f3ae3a24e4e0071558a2e2e4a845f/examples/codex-workspace-profile.json),
SHA-256 `e68d1cf569519177243ba43eefb08e14a7e2022bcd2695bdd5ced0d1c836e903`.
The 0.162.0 comparison below refers to
[its evidence page](https://github.com/joshrotenberg/due/blob/437aea507f4f3ae3a24e4e0071558a2e2e4a845f/docs/native-trace-evidence.md).

## Environment

| Item | Value |
| --- | --- |
| Due | `due 0.1.0`, installed `~/.cargo/bin/due`, a symlink to the main checkout's `target/release/due` |
| Due executable SHA-256 | `e345866de1ddebb40e51b147982778a275d367ba8ccd2088e63e5ca4864763d2` |
| Due source | Inferred as main `3a5ad50`, not demonstrated: the binary's mtime is 75 s after that commit's timestamp, the checkout it lives in is clean at `3a5ad50`, and it embeds rustc `b940084d7` (1.99.0). A local rebuild of the same Rust source in another directory produced a different hash. Build log, hashes and timestamps are retained locally. |
| Codex CLI | `codex-cli 0.163.0`, PATH `~/.local/bin/codex` |
| Host | macOS 27.0.1 (26A434), arm64 |
| Requested model / effort | Not requested |
| Observed in native thread settings | Model `gpt-6.1-sol`, reasoning effort `ultra`, approval `never` |

Existing Codex authentication and settings were used. No credential,
configuration or permission file was edited and no approval was granted.

## Fixture

- A fresh disposable Git workspace containing `control.py` (writes
  `control-<token>.txt` in the workspace) and `probe.py` (tries to append to
  `../canary.txt`; on `OSError` appends `<token> errno=<n>` to an in-workspace
  ledger, prints `denied errno=<n>` and exits 13).
- A disposable canary file in the workspace's parent directory, outside the
  granted workspace, hashed before and after each turn.
- The eight-setting profile, including `sandbox_mode="workspace-write"`,
  `approval_policy="never"`, `network_access=false`, `writable_roots=[]` and both
  temp exclusions, forwarded unchanged. A Due `--dry-run` plan, retained
  locally, shows the full native argument list.
- Due's existing `--binary` option pointed at a local transparent capture
  helper that runs PATH `codex` with the same arguments and stdin, copies every
  stdout byte to Due and to a new exclusive private file, inherits stderr and
  preserves the exit status. A retained offline check against a fake executable
  confirmed byte identity including non-UTF-8 bytes, stderr passthrough, exit
  status, and refusal to overwrite a capture file. The helper is investigation instrumentation kept local, not a
  supported runner.
- Each turn: `due run --request <profile> --provider codex --binary <helper>
  --cwd <workspace> --stdin --output json --trace-file <new> --trace-detail
  --timeout 10m`. The prompt asked for exactly two commands in order, a
  successful control then one failing command, with no other commands, no
  retries, no approval requests and no alternative writes, plus a unique
  per-turn token.

## Per-turn correlation

"Native" is the captured raw `exec --json` stdout. "Session" is Codex's own
task-owned session record for that turn, located by the native session ID and
the turn token. "Due" is the final trace record.

| Turn | Second command | Native command completions | Session raw tool exits | Session command items | Due started / completed / failed | Class |
| --- | --- | --- | --- | --- | --- | --- |
| 1, fresh | Inline `python3 -c` write to the canary | `0` only | `0`, `1` (PermissionError, Operation not permitted) | `0` only | 1 / 1 / 0 | Native event missing |
| 2, fresh | `python3 probe.py` (actual denial, prints `denied errno=1`) | `0`, failed `13` | `0`, `13` | `0`, failed `13` | 2 / 2 / 1 | Present; Due correct |
| 3, resumed from 2 | Inline `python3 -c` write to the canary | `0` only | `0`, `1` (PermissionError, Operation not permitted) | `0` only | 1 / 1 / 0 | Native event missing |
| 4, fresh | Prints `PermissionError: [Errno 1] Operation not permitted` to stderr and exits 1, no write attempted | `0` only | `0`, `1` | `0` only | 1 / 1 / 0 | Native event missing |
| 5, fresh | Prints `plain failure` to stderr and exits 1 | `0`, failed `1` | `0`, `1` | `0`, failed `1` | 2 / 2 / 1 | Present; Due correct |

For every turn:

- Due exited 0 with status `succeeded`. The result validated against
  `schemas/run-result.schema.json` and every trace record against
  `schemas/trace.schema.json`. Sequences were contiguous, all records shared the
  result's `run_id`, the last record was `run_finished`, `trace.incomplete` was
  false and `dropped_records` was 0.
- Every captured native stdout line parsed as JSON (8 records for turns 1, 3
  and 4; 10 for turns 2 and 5). In each stream the second record, immediately
  after `thread.started`, was a native `error` item carrying an
  under-development feature warning, which Due receives unchanged.
- The control file existed with the turn's token, and the canary hash was
  unchanged across the turn.
- The final answer reported the same exit codes as the session tool outputs.

After all five turns the canary still held its original text, and the shared
ledger had exactly one entry, written by turn 2, recording errno 1.

The classification uses native stdout and the session records, not the model's
answer. No activity records were inferred from assistant prose.

## Comparison with 0.162.0

The 2026-10-13 evidence on 0.162.0 recorded the same split: script probes that
printed `denied errno=1` and exited 13 were emitted, and an inline Python write
with an uncaught PermissionError was not. That omission was seen only on a
resumed turn. On 0.163.0 it also occurs on a fresh turn (turn 1). Turn 4 is new:
it shows the omission without any actual denial.

## Source note

The published
[`rust-v0.163.0` `denial.rs`](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/sandboxing/src/denial.rs#L13-L72)
treats a nonzero exit under an active sandbox (sandbox type not `None`) as a
likely sandbox denial when stdout, stderr or aggregated output contains one of
seven keywords, including "operation not
permitted" and "permission denied". Exit codes 2, 126 and 127 are excluded only
in the non-keyword path. Callers in
[`unified_exec/process.rs`](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/core/src/unified_exec/process.rs#L336-L348)
and [`exec.rs`](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/core/src/exec.rs#L828-L836)
route such outputs to a denial path instead of the ordinary output path. The
observations fit that heuristic. This page does not establish whether the
denial path causes the missing item emission, and a matching version string does not prove the installed
executable was built from that tag.

## Limitations

- One turn per shape on one machine, model and effort. No frequency or
  determinism claim. Turns 4 and 5 used `zsh -lc` and `zsh -c` respectively, as
  chosen by the model; the other turns used `-lc`.
- Exit code and output text are not fully crossed. The only exit-13 failure was
  turn 2, without a keyword; no keyword-bearing exit-13 failure was tested.
- Traces remain observations, not a complete command or permission audit. Zero
  failed activities and zero dropped records do not show that no command failed.
- Raw prompts, native stdout, session records, traces, session IDs and local
  paths remain private on the machine that ran them.
