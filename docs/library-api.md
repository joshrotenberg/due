# Rust library API audit

This document records an audit of the `due-core` public API and a proposal
for the surface a stable 0.x release would commit to. It covers the first step
of [issue #95](https://github.com/joshrotenberg/due/issues/95). The decisions
on a stable API and on crates.io publication remain open.

The audit was made against commit `b789e97` with the pinned wrappers
`claude-wrapper 0.14.11` and `codex-wrapper 0.4.9`, Claude Code 2.1.317 and
codex-cli 0.165.0 on macOS. The baseline for source consumers is
[Core library spike](core-spike.md#source-use-and-api-status).

## Entry points

| Entry point | Feature | Unit | Returns |
| --- | --- | --- | --- |
| `oneshot::execute(&RunOptions, prompt, observer, cancel)` | `oneshot` | one native turn in a child process | `RunResult`, never an `Err` |
| `Engine::new(Limits)` | `resident` (default) | an owner of resident workers on the current Tokio runtime | `Result<Engine, Error>` |
| `Engine::spawn`, `follow_up`, `fork` | `resident` | one assignment on a fresh, live or saved worker | `Result<RunRef, Error>` |
| `Engine::inspect`, `wait`, `interrupt` | `resident` | observation and control of runs | `RunSnapshot`, `WaitResult` |
| `Engine::subscribe`, `respond` | `resident` | session item stream and native interaction replies | `SessionStream`, `ResponseReceipt` |
| `Engine::release_run`, `release_session`, `shutdown` | `resident` | reclaiming capacity and settling workers | `Result<(), Error>` |

Consumers in this repository: the `due` CLI uses `oneshot::execute` for
`due run` and the hidden `legacy` module for RPC sessions. `due-mcp` and the
`due-core-compare` example use `Engine`.

## Findings

### Input validation

The two entry points validate different sets of inputs. The resident engine
checks more, and the CLI adds its own checks in front of one-shot. Rust
callers of one-shot get neither.

| Input | `Engine` | `oneshot::execute` | Issue |
| --- | --- | --- | --- |
| Empty or whitespace prompt | rejected | rejected | |
| Prompt over 1 MiB | rejected | accepted | [#207](https://github.com/joshrotenberg/due/issues/207) |
| Empty Codex model | rejected | panics in `codex-wrapper` | [#207](https://github.com/joshrotenberg/due/issues/207) |
| Empty or whitespace model or resume ID | rejected | passed to the native CLI | [#207](https://github.com/joshrotenberg/due/issues/207) |
| Model or native arguments over 1 MiB, more than 4096 arguments | rejected | accepted | [#207](https://github.com/joshrotenberg/due/issues/207) |
| Resume ID starting with `-` | rejected by `fork` only | passed to the native CLI | [#206](https://github.com/joshrotenberg/due/issues/206) |
| Zero or overflowing timeout | rejected | rejected | |
| Resume together with ephemeral | rejected | rejected | |
| Non-UTF-8 paths, missing cwd | rejected | rejected | |
| Native argument policy | resident allowlist (`session/policy.rs`) | passthrough with reserved options (`oneshot/policy.rs`) | intentional, documented |

The one-shot rows were observed with dry runs. A resume ID starting with `-`
is placed as its own argv token. Claude declares `-r, --resume [value]` with
an optional value and Codex declares `[SESSION_ID]` as a positional, so both
CLIs parse such an ID as an option. For resident Claude workers and RPC
sessions, this is a way around the native argument allowlist.

Two smaller differences are not filed as defects:

- A missing executable is an `ErrorKind::NotFound` result in one-shot. The
  resident engine defers it to launch, where it becomes a failed run.
- One-shot error messages name CLI flags (`--cwd must name a directory`,
  `pass the prompt with --prompt or --stdin`, `use --resume or --ephemeral
  before --`). Library callers set fields, not flags.

### Runtime and cleanup ownership

| Question | `oneshot::execute` | `Engine` |
| --- | --- | --- |
| Runtime | any Tokio runtime with process, I/O and time drivers, used through the wrappers; the crate does not depend on Tokio for this feature | captures the current runtime in `Engine::new`; fails with `ErrorKind::Runtime` outside one |
| Caller cancellation | the `cancel` future: the wrapper sends SIGINT, waits 2 seconds, then kills the process group | `interrupt`, the assignment timeout, or `shutdown` |
| Dropping the future | the wrapper's `kill_on_drop` and group-kill guard send SIGKILL to the process group, without the grace period | dropping `wait` or a response future leaves work running; dropping `Engine` requests best-effort cleanup on the captured runtime |
| Confirmed cleanup | the returned `RunResult` | `shutdown`, `release_session_with_report` |
| Blocking work | `Context::resolve` canonicalizes paths, searches `PATH` and reads the schema file synchronously inside the async function | admission validation is synchronous inside async functions |

The resident guarantees are covered by `crates/due-core/tests/core.rs`.
One-shot behavior is covered through the CLI's process tests. No test calls
`oneshot::execute` from Rust, and no test drops its future mid-run. The drop
behavior above comes from the wrappers' source and their own tests.

### Provider parity

Both one-shot providers map wrapper timeouts and cancellations to the same
`ErrorKind` values and fill the same `RunResult` fields. Differences found:

- With an observer (streaming), a successful Codex run has empty
  `diagnostics`, because the wrapper's streaming API discards stderr on
  success. Buffered Codex runs and Claude runs keep stderr. The code notes
  this in `oneshot/codex.rs`.
- A Codex dry run creates and removes a temporary directory for the final
  message path. A Claude dry run touches nothing.
- In the resident engine, Codex resume and fork go through app-server
  JSON-RPC parameters, and Claude resume goes through argv. Only the Claude
  path is affected by [#206](https://github.com/joshrotenberg/due/issues/206).

### Public surface

These items constrain what can change without breaking a consumer:

- No public type is `#[non_exhaustive]`. Adding a field to `RunOptions`,
  `WorkerOptions`, `Limits` or `RunResult`, or a variant to `ErrorKind`,
  `Status`, `RunState` or `ActivityKind`, breaks callers that build the struct
  with a literal or match the enum exhaustively.
- There are two public `ErrorKind` types. `due_core::ErrorKind` (operation
  errors, re-exported from the resident module by a glob) and
  `due_core::result::ErrorKind` (run failures). With the `resident` feature
  off, `due_core::ErrorKind` does not exist.
- `RunError` implements neither `Display` nor `std::error::Error`.
  `RunError::exit_code` returns the CLI's process exit code mapping.
- `RunResult` carries the CLI JSON contract: `schema_version`, `trace`,
  `invocation`, `answer_schema_validation`, and a hand-written `Serialize`
  implementation that defines the wire shape. It has no `Deserialize`.
- `Provider::from_str` and `Effort::from_str` are inherent methods with an
  `ignore_case` flag and a `String` error, not `FromStr` implementations.
  `types::parse_timeout` parses CLI duration strings.
- `RunOptions::schema` is a path. A library caller must write a JSON schema to
  disk to request structured output.
- `diagnostics`, `provenance` and `legacy` are `pub` but `#[doc(hidden)]`.
  `due-cli` re-exports `diagnostics` and `legacy`.
- `WorkerOptions` and `SavedSession` implement `Deserialize` with
  `deny_unknown_fields`. A caller that persists a `SavedSession` and reads it
  with a different Due version fails when a field was added or removed.
  `RunRef`, `SessionRef` and `InteractionRef` also deserialize, so their
  serialized form is part of the surface.
- `RunObserver` gives every method a default body, so methods can be added
  without breaking implementors.

## Proposal

This proposal is input for the decision in #95, not the decision.

### Surface

A stable 0.x API would cover:

- `Provider`, `Effort`, with `FromStr` and `Display`.
- `oneshot::{execute, RunOptions, RunObserver, ActivityKind, ActivityStatus}`.
- `Engine` and its methods as listed above, with `WorkerOptions`,
  `Assignment`, `FollowUp`, `Limits`, `InteractionMode`, `WaitOptions`,
  `WaitUntil` and the snapshot, reference, stream and error types.
- A library result type: status, text, structured output, native session ID,
  usage, cost, duration, error, diagnostics and provenance.

It would leave out or move to `due-cli`:

- `RunError::exit_code`, `types::parse_timeout`, the `RunResult` wire
  serialization, `schema_version`, `invocation` and `trace`.
- The hidden `diagnostics`, `provenance` and `legacy` modules. `legacy`
  can remain hidden until the RPC adapter moves to `Engine`.

### Compatibility rules

- Mark public enums and option structs `#[non_exhaustive]`, and keep
  `new` constructors plus public fields for setting options.
- Keep one operation error type and one run failure type, with distinct
  names, both implementing `std::error::Error`.
- Add `#[serde(default)]` to fields of persisted types, and decide whether
  `deny_unknown_fields` stays on `SavedSession`. If it stays, state that saved
  sessions are tied to the Due version that wrote them.
- Treat MSRV (1.90) increases and feature renames as breaking in 0.x minor
  releases.

### Publication prerequisites

- The workspace sets `publish = false`.
- `claude-wrapper` and `codex-wrapper` are pinned with `=` versions. A
  published library with exact pins forces the same versions on every
  consumer. Publication needs caret requirements, or a decision that exact
  pins are the price of the parsing contract.
- `due-mcp` pins `tower-mcp = 0.25.0` exactly, with the same concern.
- The crates.io API returned 404 for `due-core`, `due-mcp` and `due` on
  2026-11-12, so the names were unclaimed on that date.

### Order of work

1. Fix [#206](https://github.com/joshrotenberg/due/issues/206) and
   [#207](https://github.com/joshrotenberg/due/issues/207). Both apply whatever
   the publication decision is.
2. Add a Rust test that calls `oneshot::execute` against fake providers,
   including a dropped future.
3. If the surface above is accepted, make the public surface changes in one
   breaking revision while the crate is unpublished.
4. Decide publication after that revision has a second consumer outside this
   repository.
