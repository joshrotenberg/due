# Rhai spike assessment (historical f6aec6e)

This assessment describes the completed Rhai-only base. The Lua comparison
branch changes the batch host to shared run handles and real completion-order
collection; see [the language comparison](workflow-language-comparison.md). Measurements and
independent reviews below have **not** been repeated or attributed to this branch.

Recommendation: keep the experiment, then simplify the boundary before promoting
it. The concrete result supports a script library over a small execution host.
It does not justify making the current workflow manifest or mandatory contract
policy permanent.

## What this slice demonstrates

A feature-gated `due workflow check/run` host executes ordinary Due children from
the same executable. Rhai implements the composition: parallel reviews,
synthesis, conditional follow-up, and substitutable local components. The stable
CLI and installed binary remain unchanged.

The caller independently exercised nine black-box cases through synthetic native
Claude/Codex executables: staged parallel execution, failed answer gating,
whole-batch budget rejection, duplicate IDs, effect-free check and custom module
substitution, malformed profile rejection, VM operation limits, final output
validation, and cancellation. All passed. Cancellation stopped both active native
children, never launched the queued child, and returned 130. Raw child envelopes
were retained alongside independent validation; checked receipt permissions were
0700 for directories and 0600 for files.

A real read-only run used Claude and Codex to review a synthetic calculator, then
Claude reconciled the results. Both found the single planted subtraction bug.
All three answer contracts passed independent Draft 2020-12 validation; all child
traces were contiguous and complete, with zero dropped records. Their durations
were 9.895 s, 11.805 s, and 5.834 s. The review processes overlapped; synthesis
started after both ended. The input workspace's file hashes were unchanged.
A second real pass after the first review corrections also passed (6.229 s,
13.322 s, 7.302 s), including the revised child argument and trace handling.
The later failure-path corrections were verified with synthetic regressions.
Provider/model/effort defaults were used; normalized results did not establish
observed model identities. This checks composition, not model superiority.

## Costs and comparison

On this macOS arm64 host, Rust 1.99 release binaries from the same base measured
3,151,072 bytes without the feature and 12,405,840 bytes with it (3.94x).
Normal dependency entries including Due grew from 61 to 119. These are local
measurements, not stable size or build-time promises. Cargo 1.90 invoked the host PATH compiler
(1.99); explicit compiler-path checks separately verify the 1.90 MSRV.

An external Node baseline completed the same four-step synthetic graph and
returned the same value: two parallel reviews, synthesis, conditional follow-up.
Its 69-line caller used the existing Node artifact example and simple assertions
for the fixture answer. It did not implement equivalent general schema checking,
VM budgets or workflow cancellation. Rhai makes shared guidance and components
part of the installed binary, but is not intrinsically less code than JavaScript
for a caller already comfortable with Node. The host adds roughly 1,300 Rust
lines, plus tests/docs/dependencies. This is a meaningful maintenance cost.

## Contract policy belongs above execution

The current host requires input, per-profile answer and final-output contracts.
That is useful for testing gates, but mixes a particular workflow policy into
execution. The next iteration should distinguish mechanisms from policy:

| Layer | Responsibility |
| --- | --- |
| Ordinary Due execution | Provider invocation, native permissions, cancellation, exit status, normalized result/events, raw answer presence and native schema transport when requested. |
| Small host primitives | Bounded execution/join, artifacts, logging, checked JSON conversion, and an offline schema-validation primitive. |
| Rhai components | Optional input/output contracts, required-answer gates, acceptance criteria, composition, and mapping raw results to a workflow's request/response contract. |

A basic script should be able to return a raw result without adopting a workflow
contract. A reusable contracted component can opt into validation and expose only
validated answers. Failure should remain explicit data until the component
chooses to stop, branch or request more evidence. Resource/permission controls
and accurate execution receipts stay in Rust; scripts cannot override them.

The existing `ok` field currently combines execution and contract success. Do
not silently weaken that promise: a future raw-run primitive and a contracted
component need distinct outcomes, with validation evidence produced by the host
primitive and acceptance policy chosen by the component. Claude's review favored
mandatory host gates; the caller recommends testing this explicit separation
instead of making either policy universal.

This does not remove useful JSON transport or request profiles from the CLI.
Those are stable input/output mechanisms, distinct from whether a task result is
acceptable. Existing CLI contracts should remain compatible while a scripted
alternative is evaluated. No migration or deprecation is part of this spike.

## Boundaries to rework or retain

Keep ordinary Due subprocess execution and joined cancellation. Keep caller-owned
profiles/workspaces and a finite run budget. Keep pattern libraries in Rhai.
Rework mandatory contract policy, the custom reserved-name lexer, and module
restrictions before calling the scripting surface stable. The lexer makes
ordinary syntax and data keys part of an extra language policy; that cost needs
a concrete justification rather than becoming permanent by accident.

The spike joins concurrency-sized chunks; it is not a work-conserving scheduler.
Its workflow log reports phase/admission/completion, while native progress lives
in linked child traces. A slow child delays sibling completion records until the
chunk joins. Workflow records lack elapsed timestamps. Cleanup waits for Due's
wrapper cleanup without a forced-kill fallback; escaped descendants are outside
this host's accounting. These are explicit limits, not evidence of production
readiness. There is no server, durable scheduler, resume, retry, registry, workspace
manager or new native permission policy.

For source testing, keep feature builds in a separate target directory. Cargo
reuses the binary path across feature configurations; default tests replaced an
earlier feature binary during this trial. This is a caller/build setup issue,
not a Due dispatch failure.

## Independent review and corrections

Codex implemented the slice through Due. Claude independently inspected a frozen
snapshot through Due with read-only tools. It reported one medium receipt defect
and four low findings; there were no high findings. The caller reproduced the
medium defect: exhausting the event budget after two admissions produced two raw
child results but only one recorded step. The corrected loop retains every joined
sibling even when recording fails. Regression coverage also checks blocked-check
signals, host output-limit causes, and the scope of the child trace limit.

Corrections preserve the stop cause and originating step in the workflow error,
keep normal OS signal handling during check, replace an inherited trace-limit
environment variable with an internal child argument, and align skill metadata
and exit-code documentation. A narrow follow-up review found that secondary
receipt failures could still overwrite the original cause, and some host setup
failures were treated as ordinary child failures. The caller corrected both with
explicit host/protocol/cancellation error classes and first-cause preservation.
Combined-error and artifact-collision regressions passed. These final corrections
were verified by the caller, not a third model review. Default run behavior stays
unchanged. Child trace
completeness remains separate evidence; a valid answer does not prove a complete
trace or correct workspace effects.

The caller ran all local checks: 135 feature-enabled tests (20 workflow integration
tests), 114 default-build tests, both Clippy configurations, formatting and the
existing Node caller tests. Compilation of all targets with the feature was also
checked using explicit Rust/Cargo 1.90 paths. The implementation worker additionally
ran both full suites with explicit 1.90 toolchain paths. CI configuration includes
the experimental feature, but these are local results; remote CI is not claimed.

The independent caller probe initially used timing to assert native overlap. A
very fast fixture could finish before the second native process started despite
concurrent Due children. Replacing this assumption with an explicit readiness
barrier made the check deterministic; all nine cases then passed. No runtime
change was needed for that probe correction.
