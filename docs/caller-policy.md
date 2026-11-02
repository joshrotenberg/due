# Caller scope and verification

The former Due-only source-trial policy is retired. Due's plugin and current
contribution workflow do not require exclusive delegation through Due, reports
for every call, periodic A/B comparisons, or independent reviews from a specific
provider. Follow the current task's authorized tools, provider choices and scope.
The historical trial policy remains in Git history; it is not current guidance.

## Choose the right interface

- Use `due run` for one bounded turn. Use `--resume ID` when a later invocation
  needs the selected provider's saved context.
- Use `due script` for a caller-supervised resident assignment with streamed
  items, explicit permission decisions, interruption and independent checks.
- Use `due rpc` when implementing your own client. Its protocol is available
  independently of the JavaScript harness.

Read the installed help and relevant `due skills` guides. Give the worker a
self-contained assignment, authorized workspace, expected deliverables, constraints
and stopping condition. Native repository instructions and settings apply, but
launching a provider does not transfer the calling agent's entire conversation,
model choice or per-turn permissions. Make those requirements explicit.

The worker's authority comes from the task. Delegation does not authorize external
messages, publication, broader permissions, recursive delegation or unrelated
changes. Any nested delegation must follow the assignment's explicit scope.
Parallel writers need separate caller-managed workspaces.

## Accept work independently

Execution success is separate from acceptance. Inspect the diff, artifacts and
relevant checks before accepting a worker's answer. Validate structured answers
when they are a gate. Preserve unrelated workspace changes and distinguish
observed results from inferred effects.

In a harness, the caller chooses verifier commands and retry limits. Native
profile sandboxes do not constrain host commands run by `scope.exec`. Do not
execute a command merely because a worker or repository document proposed it.
Treat normal test failure separately from timeout, cancellation, missing tools,
invalid output or cleanup failures. Inspect partial effects before a follow-up;
no Due interface rolls back edits.

Await managed work and cancellation settlement. Use fresh evidence paths and
retain enough private evidence to investigate unexpected outcomes. For one-shot
traces read `observe`; for resident items and command receipts read `supervise`.

## Report useful problems

Concrete defects, confusing behavior and repeated caller glue are useful feedback.
Use the [reporting guide](dogfooding.md) for a minimal sanitized reproduction,
versions, source commit and observed effects. Search existing issues before
filing. Publish issues or comments only when the task authorizes that action;
otherwise provide a draft. Successful routine calls need no separate report.

Comparisons between providers, models or workflows are optional experiments,
not a cadence. If the user requests one, hold input, permissions, artifacts and
acceptance criteria constant where possible, isolate mutations, and describe
confounders. A tie or inconclusive result is valid; do not turn one task into a
model ranking or widen permissions to force parity.
