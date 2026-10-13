# Shared-host sibling cancellation (#19)

Candidate repair based on composition spike
`016837018aaf6765a5b758976f1e792f4ef100ad`, investigated on macOS arm64 with
Rust 1.90. This is an experimental workflow-host race reproduced with synthetic
native protocols. It is not a finding against a particular workflow language or
an observed stable `due run` failure. Independent Claude review approved the repair;
this draft does not close [#19](https://github.com/joshrotenberg/due/issues/19).

## Cause and observed state

Rust 1.90's Darwin implementation creates a pipe and then sets `FD_CLOEXEC` on
its two descriptors in separate operations. Its command spawn path sets up
stdio before `posix_spawn`, without closing every unrelated descriptor in the
child. A concurrent spawn can therefore inherit another spawn's pipe writer
before that writer becomes close-on-exec. See the pinned standard-library
[pipe implementation](https://github.com/rust-lang/rust/blob/1.90.0/library/std/src/sys/pal/unix/pipe.rs#L13)
and [spawn implementation](https://github.com/rust-lang/rust/blob/1.90.0/library/std/src/sys/process/unix/unix.rs#L65).

The workflow host starts Due children from separate Tokio tasks. In the caller's
controlled reproduction, a live B held the **same pipe endpoint** as A's stdout,
as well as another pipe pair belonging to the completed readiness child. The
readiness child's full JSON result and terminal `run_finished` trace existed,
but its outer `exit.json` and `step.json` did not. Its process had exited; the
host's capture still needed EOF, which B's inherited writer prevented. Thus
`wait(ready)` could not return, `cancel(a)` had not yet executed, and the outer
fixture eventually timed out waiting for A's receipt. A missing A receipt alone
does not identify which earlier stage is blocked.

The caller reproduced this with both synthetic Claude and Codex protocols.
The checked-in injector also reproduced it independently with both Rhai and Lua:
at failure the readiness result was complete (420/421 bytes) with a terminal
trace, while A and B were active and none of the three outer step receipts
existed. Failure snapshots were taken before signalling cleanup. Cleanup then
produced cancelled A/B receipts and a succeeded readiness receipt, joined the
workflow with exit 130, and left none of the recorded native PIDs alive.

The interposer only extends the existing `pipe()`-return-to-`fcntl` window in
the workflow process. It does not clear flags, fabricate data, modify signals,
or delay native `run` children. This reproduces the race class and an external symptom matching the report. The original unsaved intermittent failures cannot be assigned
this cause retrospectively with certainty. Initial uninjected probes passed;
those passes did not establish a repair.

## Repair boundary

On macOS, `process::with_spawn_lock` serializes the entire synchronous spawn
operation, including stdio pipe creation and exec completion. It releases the
process-wide lock before request delivery, capture, cancellation polling, or
child waiting. Jobs still overlap at the configured capacity, and collection
still drains through EOF. The cancellation check is inside the lock so a job
cancelled while awaiting the spawn boundary does not start afterward.

Both production spawn sites in the experimental host use the boundary:

- `process::run_inner` starts ordinary Due jobs and is the demonstrated race site.
- `fennel::isolated_compile` starts the compiler helper. Compilation currently
  finishes before job admission, but this spawn also uses the boundary: even a
  spawn using file-backed stdio must not inherit another spawn's temporary pipe.

Signal/runtime initialization precedes these job spawns. The pinned native
wrappers execute in separate Due processes, each running one provider operation;
they do not participate in the parent's concurrent pipe-setup window. No wrapper
pin or provider adapter change is needed. Main, the installed CLI, permissions,
native cancellation grace, workflow capacity and readiness guards are unchanged.
The lock is macOS-only; this is not a general replacement for OS descriptor
inheritance rules. Future in-process spawn/pipe sites must be audited: code that
does not participate in this boundary is not protected by it.

## Regression and evidence

`tests/fixtures/workflow-pipe-window.c` is a small Darwin dyld interposer compiled
by the integration fixture with `/usr/bin/clang`. Per-command environment selects
the library for a synthetic workflow; production has no test flag. The test
asserts that injection actually ran, uses four Tokio workers, and widens stdout
pipe windows by 210–310 ms. It exercises real child processes and receipt EOF,
not the existence of a mutex. Scheduling remains OS-controlled; the injected
window is the controlled perturbation, not a claim of exhaustive interleavings.

The exact `darwin_pipe_window_does_not_block_sibling_cancellation_{lua,rhai}`
tests both failed **before** the runtime edit at the unchanged ten-second A
receipt guard (25.59 seconds including both failure cleanups). Both passed with
the spawn fix (6.49 seconds total). The original polling-barrier test is retained.
Separate coverage proves complete cancelled/130 receipt contents, repeated
cancel/wait identity, partial workspace effects and trace activity, A's native
cleanup and joined descendant, and B answering a fresh challenge before release.
A deliberate fixture panic verifies joined A/B cleanup, a never-launched queued
reservation, and retained pre-cleanup artifacts. Receipt polling waits for the
final newline within the same ten-second guard: file creation alone can expose
incomplete JSON, a separate test-reader race observed during investigation.

Local private evidence is under `target/issue19-evidence/`:

- `pipe-before.log`, with retained Lua and Rhai fixtures containing
  `before-cleanup/`, final artifacts and `test-cleanup`.
- `pipe-after.log` and the focused validation logs. The earlier `probe.log`
  records uninjected probes and the separate partial-JSON reader observation.
- Caller evidence was read from its `caller-pipe-evidence.md`, process/descriptor
  captures and pre-cleanup snapshots. Raw machine paths and process tables remain
  private; the caller owns further provider comparison, review and publication.

Builds use the matching Rust 1.90 `PATH`, explicit `RUSTC`/`RUSTDOC`,
`--locked --offline`, and checkout-local `CARGO_TARGET_DIR=target/caller`.
Final focused checks used `--features workflow-spike,workflow-fennel-spike`:

| Check | Result | Private log |
| --- | --- | --- |
| Complete `workflow_handles`, four test threads | 12 passed | `handles-final.log` |
| Fennel/Lua v2 sibling cancellation | 1 test passed, both languages | `fennel-sibling.log` |
| Composition cleanup tests matching `join` | 3 passed | `composition-join.log` |
| Fennel composition and component locations | 1 passed | `composition-fennel.log` |
| Clippy, all targets, warnings denied | Passed | `clippy.log` |
| Stopped-compiler signal inspection | Ignored by its existing sandbox restriction | `compiler-signals.log` |

Reproduce the injected pair with `cargo test --locked --offline --features
workflow-spike,workflow-fennel-spike --test workflow_handles darwin_pipe_window
-- --test-threads=1 --nocapture`, using the toolchain/target settings above.

The implementation worker used synthetic protocols and did not make nested model
calls, install tools, or publish changes. Its sandbox denied process inspection
with `ps`; recorded identities, receipts and `kill(pid, 0)` checks supplied local
cleanup evidence. The outer caller separately inspected owned process descriptors
and ran the provider review through installed Due.

These before/after results are local macOS arm64 evidence. Hosted/platform CI has
not verified this repair; prior spike runs were rejected by GitHub’s account
payment/spending-limit gate. Local success is not a hosted CI result.


## Final caller verification

The full local matrix passed: default **114**, Rhai **149**, Lua **160**,
Fennel **183**, and all features **205** tests. Fennel/all each skipped the same
process-inspection case in normal execution; the caller explicitly enabled that
case with inspection available and it passed. All five all-target Clippy runs
passed with warnings denied, plus formatting, whitespace checks and the Node
caller's 22 tests. Counts overlap across configurations.

The caller replayed its independent pipe-window injector against the final
candidate in all four Lua/Rhai × synthetic Claude/Codex combinations. All passed
with the original ten-second guard: cancelled A receipts arrived in 2.861–3.144
seconds while B remained blocked, replacement work progressed, partial effects
and canaries matched, and native processes were gone at invocation completion.
The four cases validated 16 child result/receipt sets and 110 native trace records.
These are synthetic protocol checks, not real-provider cancellation claims.

Through installed Due, Codex performed the investigation/repair and Claude
independently reviewed the immutable source snapshot. Claude approved with no
required correctness changes. Documentation notes were addressed; optional
spawn-lock poisoning recovery was deferred because no current closure has an
observed panic path and recovery semantics are outside this focused repair.
Only documentation changed after that review. The caller independently validated
successful answer/result contracts and all trace schemas/sequences. A deliberately
interrupted investigator and a rejected native resume invocation remain separate
partial/failed outcomes, not successful work. The permission-preserving resume
correction is recorded in [#26](https://github.com/joshrotenberg/due/issues/26).
