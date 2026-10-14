# Finocchio decision plan

Status: bounded experiment plan, 2026-10-13. Chunk 0 and an isolated integrated
module/workflow slice are implemented. The slice demonstrates part of chunk 1's
CLI/library boundary and chunk 2's synthetic ownership gate. It does not complete
chunk 1's findings tool/tooling trial, chunk 2's live provider gate, or chunk 3.
No change authorizes an unfreeze or pauses another project's work.

Implemented slice: `workflow run/describe/check`, ordinary reusable agent modules,
the same Finocchio compiler/data/contracts in the integrated VM, caller-owned
profiles/roles/workspaces, finite review/revise policy in Fennel, ordinary stage
replacement and synthesis, shared admission limits, cooperative cancellation and
retained receipts. Tests execute real Fennel against synthetic Due-compatible
children, including barriers, both OS signals, library cancellation/drop and
hostile output/pipe cases. All 36 prior tests remain. The root binary-only Due
boundary requires a small duplicated CLI adapter; see its
[maintenance and limits inventory](integrated-workflow.md). Installed Due and
root sources/Cargo/vendor remain frozen. A
[minimal live pair](live-trial.md) is prepared for the outer caller, not executed.

Remaining counterevidence includes generated-Lua rather than general macro
callsite diagnostics, selected VM controls rather than a universal sandbox,
no hard live trace spool quota, no native cleanup ownership after Due exits and
leaves unknown writers, and dependence on cooperative Due shutdown. These must
remain visible in any product decision. No A/B or independent model review was
performed by this worker.

The decision is where the reusable agent-and-tool language should live. Due's
existing single-run CLI remains useful under either outcome. Finocchio needs to
justify owning a broader language surface through working programs and maintenance
evidence. Squero is an existing BEAM language/workbench experiment, not merely a
future name for this work.

## Candidate outcomes

| Outcome | Product boundary | Evidence needed |
| --- | --- | --- |
| A: continue the previous Due workflow plan | Due gains the bounded Lua workflow API, with optional Fennel authoring. Finocchio remains a language experiment. | Existing script composition meets the real tasks; extra language semantics and tools bring too little benefit for their upkeep. |
| B: make Finocchio the higher-level target | Due retains provider execution and its single-run contract. Finocchio owns language, libraries, tool composition and agent workflows over an explicit adapter to Due's execution capabilities. Halt the duplicate planned Due workflow product surface. | Ordinary tooling and agent workflows share a useful, discoverable language; callers need no second subprocess coordinator; compiler and semantic maintenance costs are acceptable. |
| C: stop or narrow the extension | Keep stable Due and preserve experimental evidence. Reconsider Fennel's role or use Squero for the unmet requirement. | A must-have semantic, diagnostic, ownership or authoring requirement fails, or meeting it means an unaccepted compiler/runtime maintenance burden. |

Choosing B does not by itself select Finocchio as a replacement for Squero. A
local process-scoped tool can be a useful intermediate target, and perhaps a
sufficient alternative for a selected application, without providing BEAM process
semantics, supervision, code loading or distribution.

## Chunk 0: language contract and compiler boundary

Deliver [the language contract](language-contract.md), executable binding and
provenance probes, and [the compiler-boundary report](compiler-boundary.md). Keep
existing declaration behavior, runtime isolation and compiler-budget regressions.

The report must distinguish public APIs, owned conventions, version-dependent
internals and proposed upstream hooks. Test shadows, repeated and nested macro
calls, generated descendants, passed-through user expressions, macro failure,
runtime failure and shifted UTF-8 source. Negative results count as evidence.

Exit gate: every claim has a reproduction or is labelled proposed/unknown. An
explicit dependency convention must not be called definition-site hygiene.
Related diagnostic locations must not be presented as exact error origins.
Record the smallest missing compiler hook and its prospective upkeep before
adding language breadth. A compiler fork is a separate decision, not an implicit
fix for a failing probe.

## Chunk 1: one useful language tool

Chunk-0 scope decision: exact origin tracking for arbitrary third-party macros is
not a prerequisite for entering this experimental chunk. Related, partial locations
are provisionally acceptable only if the supported-program repair gate below
passes. If an author cannot locate and repair an ordinary supported failure from
the public diagnostic, return to the compiler-hook decision before expanding scope.
This is a bounded experiment policy, not a waiver of a future product requirement.

Create an isolated executable surface for a small program plus invocation input.
Proposed commands are `run`, `check` and `describe`; names and wire envelopes are
to be settled by this chunk's contract. A zero-agent program must need no provider
profiles, agent binaries or per-step answer schemas. Resolve explicit local or
embedded modules through one documented mechanism.

Add only the data operations needed by a reference tool that validates and
transforms a nested findings document: path access, immutable update, iteration
and a projection/destructuring mechanism. Use ordinary functions wherever possible.
Keep native Lua values and JSON values explicit at the boundary; preserve null,
missing, empty collection and large-number rules from chunk 0.

Give errors a machine-readable category and supported source locations. Expose
library signatures, examples and declared effects through `describe`. Checking
must not launch agents or grant runtime capabilities; document that compiling
permitted macros still executes compiler code.

Exit gate: the tool runs from outside the source directory with only source and
input, produces a validated result, and repairs from a deliberately malformed
program using public diagnostic output. Test actual CLI exit/output behavior,
module boundaries, multiple returns and meaningful negative cases. Report any
host failures still lacking structured diagnostics. Full source rename/LSP
support is not an acceptance prerequisite for this tool.

## Chunk 2: the language and agent harness together

Run the same Finocchio contracts, values, modules and diagnostics inside an owned
agent workflow. Preserve an explicit Due adapter and reuse its execution and
cleanup behavior; do not implement a second Claude/Codex protocol stack. Select
subprocess versus in-process reuse explicitly, with receipts and failure evidence.
The prior separate async probe does not establish this integration.

The reference operation is review -> conditional revision -> review, with a finite
attempt limit. Run two unchanged instances concurrently on caller-provided
isolated workspaces, replace one stage with an ordinary function, then synthesize
their results. A successful review requesting changes is a domain result; provider
failure, invalid output, exhausted attempts and parent cancellation stay distinct.

Exit gate: synthetic children establish overlap, ordered results, shared admission
limits, ownership, partial-result retention and cleanup on failure/cancellation.
Then use real providers for one bounded development/review pair and verify actual
files and tests. Parent cancellation must close admission and settle owned children;
catching a script error cannot grant new authority or suppress host cleanup.
No automatic mutation replay. No external Python/Node process orchestration is
needed by the workflow author. Existing tools used by child agents remain normal.

## Chunk 3: authoring trial and recorded decision

Freeze both candidates for the comparison: the previous Due Lua workflow surface
and the integrated Finocchio surface. Use matched tasks, fixtures, workspace grants,
answer contracts and failure injections. Give authors only public help/examples;
withhold reference solutions and grading fixtures. Use fresh sessions and isolated
outputs so one attempt does not teach another.

First compare the two surfaces with the same provider/model/effort. Separately,
where the source-trial cadence warrants it, compare one bounded prompt across
Claude and Codex with the same surface and rubric. Record the native settings
actually observed, unknowns and tool-access differences. Different implementer and
reviewer roles are not an A/B trial. A small trial can support a product choice;
it cannot establish a universal language or model ranking.

Tasks: build the pure findings tool; repair a contract or macro-related failure;
compose the saved review/revise component concurrently without changing its internals.
Record correctness, unsupported constructs, help lookups, diagnostic usefulness,
repairs, bespoke orchestration, elapsed time and available usage. Keep unlike token
and cache counters separate. Independently grade outputs and effects.

Exit gate: publish a decision record selecting A, B or C with counterevidence and
remaining limits. Retain one intended product direction and leave the other as an
experimental reference. A chosen direction still requires separate release,
compatibility, installation and hosted-CI gates before unfreezing or shipping.

## Decision rubric

| Dimension | Required evidence | Stop/reconsider condition |
| --- | --- | --- |
| Semantic correctness | Contract tests and real programs agree on values, calls, exports and errors. | Hidden conversion, binding capture or a checker that silently accepts unsupported semantics. |
| Diagnostics | An author can locate and repair the planted failure; exact, related and unknown locations are distinguished. | Correct repair requires reading generated Lua for ordinary supported use, or guessed locations are advertised as precise. |
| Ownership and authority | The integrated host bounds jobs, preserves native permissions and completes cleanup. | A caught failure leaks active jobs, permits admission after cancellation or expands permissions. |
| Authoring value | Public discovery suffices; common operations use configuration and custom operations contain task logic. | Agents repeatedly recreate process supervision, result collection or compiler-specific workarounds. |
| Upkeep | A written inventory of owned libraries, semantic rules, hooks and pinned internals; one simulated compiler-version compatibility audit. | Substantial unbudgeted compiler fork or duplicated semantic model required for the promised surface. |
| Product fit | Both ordinary tools and agent compositions use the same Finocchio surface in outcome B. | Benefits exist only in isolated macro demonstrations or require two separate user-facing workflow systems. |

Binding-aware macros and complete provenance are different requirements. Chunk 0
may justify explicit dependency passing and useful related locations as an initial
contract. If exact origin tracking for arbitrary macros is required for the product,
record that as a hard gate and return to the compiler decision before chunk 1.
Do not quietly weaken that requirement to make an existing implementation pass.

## Squero checkpoint

Before calling Finocchio a Squero alternative, review current Squero evidence and
classify actual target applications. The local Squero README/charter/first-wave
guide at commit `c1726c555bb4d9777bc03964106b59f76cb8ebf6` already describe an
analyzed language representation, immutable values, source tooling, ordinary tool
composition and trusted BEAM supervision examples. That was a documentation
inspection of tracked files; its checkout had unrelated untracked artifacts and
its tests were not run here. Revalidate at decision time.

| Requirement | Finocchio decision implication |
| --- | --- |
| Foreground local agent/tool programs with explicit contracts | Chunks 1-3 can establish whether Finocchio is sufficient. |
| Rich compiler semantics, source preservation and incremental tooling | Compare concrete coverage and upkeep with Squero's existing work, not only Lua/Fennel familiarity. |
| BEAM-native processes/messages, supervision or Erlang/Elixir integration | Finocchio does not inherit these from mlua. Keep Squero where they are actual requirements. |
| Durable recovery or distributed execution | Require separate effect/recovery evidence in either project; a successful language spike establishes neither. |

Portable results include fixtures, contract vocabulary, failure taxonomy,
authoring tasks and workflow policy. Do not promise shared source code or identical
runtime semantics. The decision record should say separately whether Finocchio is
the Due workflow target, a Squero proving ground, or sufficient for a specific
application previously expected to need Squero.

## Work discipline

Keep each chunk isolated and reviewable. Due delegates model work; one provider
implements and the other reviews, with caller verification of tests and effects.
Sanitize favorable and unfavorable findings onto matching Due issues. Main and the
installed CLI stay frozen throughout these experiments. Execution of future chunks
and any release/unfreeze remain explicit follow-up decisions.

The subsequent [caller evidence](live-evidence.md) verifies one real
review/revise/review cycle and two full synthetic cycles in parallel. It completes
that bounded integration gate, not the remaining ordinary-tool/repair and
authoring comparison or the A/B/C product decision.
