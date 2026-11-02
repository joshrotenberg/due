---
name: delegate
description: Delegate bounded coding work to Claude Code or Codex through Due, using a one-shot run or a supervised resident assignment when the user requests delegation or an independent worker would help authorized work.
---

# Delegate with Due

This plugin is an entry point to Due's built-in guidance. The `due` executable
must be installed on PATH; the selected provider CLI must also be available.

1. Check `due --version` and `due --help`. If Due is unavailable, report the
   missing prerequisite. Use the installed binary's commands and guidance.
2. Run `due skills` to discover the installed guides. Read `run` for one bounded
   turn or CLI resume, and `supervise` for a resident assignment with streaming,
   permission decisions, interruption, or independent verify/revise steps.
   Read `contracts` for one-shot result/answer semantics and `observe` for run traces.
3. Check `due run --help` or `due script --help` for the chosen interface. Normal
   builds embed JavaScript; no Node runtime is needed. A custom client may use
   `due rpc` directly. Use only commands exposed by the installed version.
4. Honor the user's provider choice. Prepare a self-contained assignment with
   its workspace, deliverables, constraints and acceptance criteria. Carry its
   exact scope and authorization into every turn; delegation does not grant
   external actions or recursive delegation. Use JSON input and result contracts.
5. For a script, choose caller-owned verifier commands and a new evidence directory.
   Await operations and cancellation cleanup. Native profile permissions do not
   sandbox the script's host commands. Keep permission decisions explicit.
6. Inspect results and independently verify the relevant diff, artifacts and checks
   before reporting completion. A worker's success report is evidence to review,
   not proof that the requested work is correct or external effects occurred.

Keep detailed procedures in the binary's guides. Use native passthrough only
for a provider option needed by this assignment and supported by its CLI.

Report reproducible Due defects or confusing behavior using
the [issue-reporting guide](https://github.com/joshrotenberg/due/blob/main/docs/dogfooding.md).
Preserve the exit status, Due/provider versions, available source commit, and a
minimal sanitized reproduction. Submit an issue when the user's scope authorizes
issue filing; otherwise provide a draft. Inspect partial effects before any
follow-up. Do not include credentials, private task content, or raw provider logs.

The former mandatory source-trial and A/B workflow is retired. This skill does not
require Due-only delegation, periodic comparisons, or reports for every call.
