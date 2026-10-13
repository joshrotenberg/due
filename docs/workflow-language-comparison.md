# Lua / Rhai comparison

**New working direction:** the [Lua composition spike](workflow-lua-composition.md)
on base `f1e6633` exposes bounded host-managed concurrent ordinary functions,
including nested reuse and a Lua library pipeline used twice in isolated
caller-supplied profiles. Rust retains native admission, capacity, ownership,
cancellation and receipts. This makes reusable suspended functions concrete and
supports Lua as the working direction. It is local fake-native evidence, not a
new provider authoring trial, real-model acceptance, CI result or shipping decision.

The comparison and trial results below are historical. Its one-coordinating-script
recommendation and one-continuation limitation describe the earlier surface;
the new capability changes that premise without changing those measured results.

Follow-up: [Fennel authoring spike](workflow-fennel-spike.md) layers an optional
pinned compiler onto the Lua candidate. The measurements and recommendations
below describe the prior Lua comparison, not new Fennel results. Combined
features remain comparative tests, not an intent to ship both runtimes.

This is an experiment on `spike/lua-workflows`, based on the Rhai spike
`f6aec6e` / draft PR #16. Stable `main`, the installed Due binary and wrapper
Git pins are unchanged. Neither interpreter is enabled in default builds.

The result supports the proposed boundary: a script describes relationships
among runs, while Rust owns admission, native processes, output, cancellation
and receipts. Both languages passed the completion-driven experiment without
turning the script into an agent runtime.

For the then-current **one invocation, one coordinating script** product, I would
keep Rhai as the slightly simpler candidate. Lua is a credible alternative with
a smaller executable and genuine async suspension. This experiment does not
justify shipping both permanently, nor inventing resumable machinery for Rhai.
If independently suspended script functions or Lua editor tooling become central,
revisit that choice with a prototype that actually exposes those capabilities.
That earlier Lua surface deliberately exposed only one host-managed continuation.

## What was compared

Both adapters use the same Rust supervisor and ordinary Due subprocesses:

- `start(job)` admits work and returns an opaque handle immediately.
- `wait(handle)` joins and returns a cached outcome.
- `wait_any(handles)` returns the earliest Rust-observed completion, with identity.
- `cancel(handle)` requests cancellation of just that job and joins it.
- `run` and atomically admitted, input-ordered `parallel` remain conveniences.

The decisive fixture starts a slow and a fast native process, consumes the fast
completion, branches on its structured answer, starts replacement work while the
slow original remains active, and cancels or joins the remaining job. Readiness
barriers establish ordering; elapsed time is not used as the concurrency proof.
The same Rust code enforces capacity, total runs, first-error retention and
cleanup of dropped handles, script errors, early returns and queued work.

The old Rhai spike used fixed-size joined batches. The new host improves **both**
languages. Its scheduling improvements are not evidence that Lua is better.

## Concrete tradeoffs

| Question | Rhai | Lua |
| --- | --- | --- |
| Waiting | One blocking orchestration worker waits on Rust futures | `mlua` async callbacks suspend the script continuation |
| Domain handles | Cloneable Rust custom type | Rust `UserData`; equally opaque to scripts |
| JSON | Distinct arrays/maps; unit represents null | Explicit null sentinel and array identity; checked table conversion |
| Components | Functions extracted from modules; top-level code discarded | Function-table modules initialize with effects disabled; lexical captures work |
| Higher-order composition | Function-pointer/closure facilities; not the focus of this trial | A tested function-taking-function phase wrapper needs no new host primitive |
| Build | Rust interpreter | Vendored Lua 5.4.9 C runtime through `mlua` 0.12.2; no user-installed Lua |
| Embedding work | Reserved bindings use Rhai's pinned AST API (`internals` feature) | Strict data conversion, coroutine visibility, C-loop bounds and library restrictions |

Neither language is categorically richer. The earlier Lua implementation proved real
async waiting, without numerous independently suspended workflow functions.
Its coroutine library, xpcall, callback-based sorting/substitution, C pattern
matching, loaders and filesystem/OS APIs are intentionally unavailable. Ordinary
Lua functions and closures remain. Rhai uses its normal parser, including string
interpolation; the initial spike's handwritten namespace lexer was removed.

## Measured build footprint

Local macOS arm64 release builds, Cargo/rustc **1.90.0**, same source, default
release profile, no stripping and independent feature selections:

| Build | Executable bytes | MiB | Unique normal dependency entries, including Due |
| --- | ---: | ---: | ---: |
| Default | 3,147,920 | 3.002 | 61 |
| Rhai | 12,091,696 | 11.532 | 119 |
| Lua | 9,911,680 | 9.453 | 121 |
| Both | 12,806,160 | 12.213 | 132 |

Lua is about 18% smaller than Rhai in this build, while adding a native compiler
step. The normal dependency count excludes build dependencies and is not a total
supply-chain measure. `otool -L` showed system libraries only, with no external
Lua dylib. These are local build measurements, not portable size promises or
interpreter speed results. Build elapsed times shared caches and are unsuitable
for comparison. The historical Rhai-only assessment used another compiler; do
not compare its numbers directly with this table.

## Local validation

The final caller-run Rust 1.90 matrix passed: **114 default, 145 Rhai-only,
133 Lua-only and 154 combined tests**. All four `cargo clippy --all-targets --
-D warnings` configurations passed, along with formatting, whitespace checks and
22 Node caller tests. Feature suites ran sequentially with four test threads.
The intermittent earlier failure below is retained as a separate finding;
passing reruns are not presented as its resolution. Remote CI is separate evidence.

## Independent execution and authoring evidence

The caller independently ran 32 synthetic native-process scenarios across the
adapters. These cover completion/replacement overlap, cancellation without
stopping siblings, queued signal cleanup, early return/throw/VM-limit cleanup,
repeat waits, invalid handles and batches, null/empty containers/exact integers,
invalid child answers, and Lua sparse/mixed/cyclic values and unavailable APIs.
All passed. A separate before/after probe confirmed the C-loop repair below.

A [blind, bounded provider trial](https://github.com/joshrotenberg/due/issues/18) gave Claude and Codex the **same prompt and API
reference**. Each returned both a Lua and a Rhai script. All four scripts were
executed unchanged against four scenarios: replacement required, no replacement,
invalid fast answer, invalid replacement answer. All **16 executions passed**.
The caller independently checked returned data, admitted jobs, cached completion
identity and native-process cleanup. The outcome is a tie on the tested rubric.
There is no demonstrated authoring-quality advantage for either provider or
language from these two attempts. Model/effort were omitted and provider-reported
values remain unknown; provider defaults and compute are not equivalent.

Each language also ran a real read-only workflow: Claude and Codex independently
reviewed the same synthetic calculator, then Claude synthesized their answers.
All six child runs and both final answers validated, all six native traces were
complete (14 records each), raw native receipts were preserved, and neither
workspace changed. Both final answers retained the known subtraction-instead-of-
addition defect at line 3. These live runs used the pre-review binaries; the
subsequent repairs affect bounds, cancellation errors and signal reporting.
Post-repair verification uses the full test matrix and synthetic native probes,
not a repeated set of paid happy-path model calls.

## Review findings and repairs

Codex implemented the comparison through Due. Claude independently reviewed a
frozen source snapshot through Due with read-only tools. The caller verified
findings instead of treating the review verdict as proof:

- **Confirmed blocking C loops:** `table.move` could loop almost 2^63 times
  without allocating or triggering Lua's instruction hook. The caller reproduced
  a no-child workflow ignoring SIGTERM and requiring its test guard to kill it.
  Inspection of vendored Lua source found the same issue in `string.rep` of empty
  strings. Both old/new cases were reproduced. Table move/insert/remove now have
  checked 4096-item bounds; empty string repetition returns immediately. Tests
  additionally verify sparse-length cases, ordinary operations, repeated caught
  bound errors and cleanup of an already-running native child.
- **Local cancellation/write race:** the review identified an inferred path where
  cancelling before request intake completes could produce EPIPE and cancel all
  siblings. An observed cancellation plus BrokenPipe is now a step-local
  protocol error; other write errors remain immediately fatal. A real closed-pipe
  regression checks local, global and absent cancellation flags. The timing race was not reproduced
  end to end, so that distinction remains explicit.
- **Feature lint and signal tests:** the worker had already fixed the two
  single-language unreachable-pattern arms and missing native-liveness assertion
  while review was running. The caller strengthened cancelled status/exit checks
  and made signal-only errors explicit without overwriting an earlier fatal cause.

Claude approved a second focused review. Its remaining P3 observation about a
signal-induced broken pipe was corrected and covered by the same regression.

This review demonstrates why a VM instruction budget is insufficient on its own.
The restrictions and guards are choices in this embedding, not inherent limits of
Lua or a proof of a complete hostile-code sandbox. Provider resource limits and
permissions remain Rust/native-runner responsibilities.

An intermittent **shared-host sibling-cancellation test** is tracked in
[#19](https://github.com/joshrotenberg/due/issues/19).
The implementation worker saw one readiness timeout, and the caller reproduced
it in a full Rhai-only suite while other probes were running. Twelve standalone
reproductions and three repeated eight-test handle suites then passed. Subsequent
full Rhai-only and Lua-only suites also passed. That does not establish the cause
or prove a repair. Failed fixtures now retain their directories for diagnosis,
and the issue must be resolved before promoting this spike. It is not evidence
favoring either language. No processes tied to the spike checkout remained when
independently inspected. Native cleanup can still wait indefinitely for an
unresponsive child, and escaped descendants remain outside the host guarantee.

The subsequent [#19 investigation](workflow-sibling-cancellation.md) reproduces
a Darwin pipe-inheritance race and provides a shared-host spawn repair with
before/after fault-injection coverage. Historical unsaved failures remain
retrospectively uncertain; independent Claude review approved the candidate,
while the issue remains open and the feature freeze remains in force.

## Boundary to retain

Keep process/session ownership, streaming, cancellation, capacity and outcome
identity in Rust. Put sequence, branch, review rounds, quorum, replacement and
synthesis policy in versioned scripts using the same small API as user scripts.
Keep only one language if this progresses past experimentation.

Contracts remain mandatory here solely for comparison with the first spike.
A later simplification can move the **decision to require a contract or gate a
step** into a script library while retaining reusable parsing/validation and
truthful run outcomes in Rust. This branch does not change `due run` into a
request/response framework, weaken existing contracts, or add servers, retries,
resume, messaging, workspaces or a registry.

See the [usage and implementation guide](workflow-spike.md), the embedded
[workflow skill](../embedded-skills/workflow/SKILL.md), and the paired
[arrival.lua](../examples/workflow/arrival.lua) /
[arrival.rhai](../examples/workflow/arrival.rhai) examples. Private raw artifacts,
prompts and model logs remain local; repository reports contain sanitized evidence.
