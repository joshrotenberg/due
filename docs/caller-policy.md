# Opt-in caller policy for the source trial

This policy applies when the user enables it in their host's global instructions.
It is a caller workflow, not a Due runtime feature or a requirement for every
plugin user. Explicit task instructions and host permissions still apply.

## Delegate through Due

Use Due exclusively for delegated model work in every repository. Do not use
native subagent tools, other delegation plugins, or direct `claude -p` / `codex
exec` calls as substitutes. The caller may do work itself; delegation is not
required for every task. If Due or a required provider is unavailable, report the
blocker and continue independent work within scope.

Read the installed help and relevant embedded skills. Give each worker a
self-contained assignment, workspace, permission scope, output contract and
stopping condition. Use JSON results and fresh trace/artifact paths. For actively
monitored runs, omit arbitrary deadlines and judge progress from events and
effects. Preserve partial effects before any authorized follow-up; do not bypass
native permissions or automatically retry mutations.

Workers must not recursively delegate unless specifically authorized. Any
authorized nested delegation must also use Due. Disable built-in native
delegation where supported and consistent with the task's permissions. Workers
return observations to the caller; the outer caller owns issue filing, A/B
selection and publication. Workers must not duplicate those actions or start
their own experiments.

For implementation work, use the other provider for independent review when
available. Inspect the diff, run relevant checks and independently validate
answer contracts before accepting work. If the other provider is unavailable,
keep that review pending rather than claiming it passed.

## Report every finding

The user enabling this trial authorizes sanitized Due findings to be reported
to [joshrotenberg/due](https://github.com/joshrotenberg/due/issues). Record favorable
and unfavorable findings: successful workflows, friction, defects, permission
failures, recovery, missing evidence and useful workarounds. A native failure is
not automatically a Due defect; identify the layer supported by the evidence.

Search existing issues first. Add evidence to a matching issue and create a
focused issue for a distinct problem. Aggregate ordinary successes and related
observations into one trial issue per work batch, preserving every finding
without creating an issue for each tool call. Application defects belong in
their own repository when that task authorizes reporting.

Record source commit and dirty state, resolved executable/build identity where
available, Due/provider versions, host/platform, requested provider/model/effort,
observed settings if exposed, exit/result/validation outcomes, trace completeness,
verified effects and limitations. Missing model or usage metadata stays unknown.
Do not infer it from a requested alias or treat unlike token counters as
equivalent costs. See the [reporting guide](dogfooding.md).

Use synthetic examples and aggregate measurements. Never upload private work
code, prompts, customer data, credentials, authentication files or raw logs.
Repository privacy does not make those safe to share. If reporting is blocked,
retain a sanitized draft and report its location and the blocker.

## Occasionally compare two choices

On the first suitable bounded task in a work session, and roughly every five
delegated tasks thereafter, consider one two-arm comparison. Defer it when no
fair, useful task exists or the user's time/account budget precludes it, and
record why. This is a workflow cadence, not a timer, background job or server.

Use the same prompt, input snapshot, answer schema, permissions and evaluation
rubric for both arms. Deliberately vary a provider, an available model or a
supported effort setting. Record all settings and unavoidable confounders:
provider comparisons involve different model families, and equal effort labels
do not guarantee equal compute. Honor explicit user model/provider choices and
verify availability with the installed provider.

The same prompt or path list does not prove both workers can read the same
material. Supply a frozen inline snapshot, an exact readable file manifest, or a
prepared diff that the granted tools can consume. Check both arms' coverage
before interpreting the comparison; record unequal access as inconclusive. Do
not widen permissions to force parity. When summarizing issue history, include
material later comments so earlier statements are not presented as current.
Evidence: [#21](https://github.com/joshrotenberg/due/issues/21),
[#33](https://github.com/joshrotenberg/due/issues/33),
[#39](https://github.com/joshrotenberg/due/issues/39),
[#43](https://github.com/joshrotenberg/due/issues/43),
[#44](https://github.com/joshrotenberg/due/issues/44) and
[#73](https://github.com/joshrotenberg/due/issues/73).

Prefer read-only diagnosis, planning or review. Mutation trials require isolated
checkouts from the same base; never run both arms against shared mutable files
or duplicate external actions. Use fresh artifacts and identical context, and
keep each arm blind to the other's answer. Limit the trial to two arms unless
the user requests more.

Choose by independently checked correctness, contract validity, scope compliance,
actionable evidence and relevant tests first. Then consider observed time, usage
and cost where comparable. Accept the better supported response, or record a tie,
neither acceptable, or inconclusive result. Do not force a winner or generalize
one trial into a model ranking. Apply or publish only the chosen, verified work.

File an issue for each completed comparison with the task class, controlled
inputs, settings, rubric, verified differences, selected outcome and limitations.
Sanitize it under the same reporting rules. Positive and tied results are useful
evidence too. Keep feature ideas in the backlog during the freeze.
