# Caller evidence: reusable agents and concurrent workflows

Local experiment, 2026-10-13, based on `c4dc6ee` with changes confined to this
spike. Codex implemented through Due; Claude independently reviewed the candidate.
The caller inspected the code, corrected profile compatibility, verified effects,
and addressed the review's reporting/documentation findings. This is not release
or general model-quality evidence.

## Real review → revise → review

The caller invoked the unchanged `review-cycle.fnl`, `reviewer.fnl` and
`reviser.fnl` from outside the checkout, with `max_revisions:1`, `max_jobs:3` and
explicit read/write grants to one fresh synthetic Python workspace. The selected
binary was the already-installed Due 0.1.0; Claude Code 2.1.296 reviewed and
Codex CLI 0.162.0 revised. Native model/effort defaults were retained. Requested
settings and native permission arguments were recorded; observed model/effort
remain unknown. This is a bounded fixture, not a production repository trial.

| Stage | Domain result | Due duration | Trace records |
| --- | --- | ---: | ---: |
| Claude review | changes requested | 8,365 ms | 22 |
| Codex revision | changed | 26,558 ms | 23 |
| Claude review | approved | 11,146 ms | 23 |

All three exits were zero and results succeeded. The caller independently
validated each Due result, each answer against its saved schema, and every trace
record against the installed contract. Traces were contiguous with one terminal,
no drops, and `incomplete:false`. All three owned Due PIDs had exited.

The fixture started with four failing tests and one passing test. Only the intended
implementation file changed: a nested maximum became a minimum in an inclusive
clamp function. The test file and README hashes stayed unchanged. All five tests
passed when run independently after the cycle. The final workflow was `succeeded`
with domain `approved`, one revision and three job receipts. Reviews used read-only
tools and explicitly said they had not executed tests; the caller verified them.

The live cycle preceded the final first-stop-reason and pre-spawn classification
corrections. Those failure-path changes have dedicated synthetic regressions;
the successful mutation was not replayed to manufacture fresh live evidence.

## Concurrent complete cycles and cancellation

An additional caller probe ran two unchanged saved cycles with separate workspace
grants. Both reached a barrier; the second completed review/revision/review before
the first initial review was released. All six jobs settled, both revisions wrote
only their own synthetic markers, and results retained input order. This probe
is now a tracked regression alongside the earlier immediate-approval overlap test.

The required suite has 65 checks: all 36 prior checks plus 29 integrated tests.
New regressions also distinguish pre-spawn `not_started`, preserve an earlier
fatal failure through later cancellation, preserve earlier cancellation through a
later capture overflow, exercise all continuation limits, reject invented Due
validation claims, and preserve caller-owned Codex permission options.

Cancellation tests use controlled children and verify cleanup/receipts; this
batch did not interrupt a real provider mid-mutation. Graceful cleanup still
requires a cooperative Due executable. Retained trace capture is bounded, but
live spool disk use has no hard quota. Read/write labels remain unchecked caller
obligations backed by native permissions, not an OS sandbox.

## Frozen boundary and remaining work

Main remained clean at `76d33d410319d860425a2dd3609863a4ba5ca565`. The installed Due
SHA-256 stayed `5ff8d62842cf50457ff7011bf9366ddb807a157d51af92a0f52f8917630d4ef7`.
No root Due runtime, feature default, vendor source, installation or main change
was made. No release or product direction has been selected.

A small independent acceptance-design comparison is recorded in
[Due #55](https://github.com/joshrotenberg/due/issues/55); the implementation and
review roles were not that A/B trial. General authoring ergonomics, the ordinary
findings tool/repair exercise, a frozen-surface authoring comparison and the
Finocchio-versus-Due product decision remain planned work. Skill labels are still
descriptive metadata, not automatic skill installation or loading.
