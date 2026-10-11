---
name: delegate
description: Delegate a bounded coding task to Claude Code or Codex through the local Due CLI when the user requests delegation or an independent worker would help complete authorized work.
---

# Delegate with Due

This plugin is an entry point to Due's built-in guidance. The `due` executable
must be installed on PATH; the selected provider CLI must also be available.

1. Check `due --version` and `due --help`. If Due is unavailable, report the
   missing prerequisite. Use the installed binary's commands and guidance.
2. Run `due skills` to discover its guides, then read `due skills show run`.
   Read `due skills show contracts` for structured answers, result fields, or
   error handling; read `due skills show observe` for progress and trace logs.
3. Consult `due run --help` for the authoritative flags. Honor the user's
   provider choice and prepare one bounded assignment with its working
   directory, expected deliverables, constraints, and completion criteria.
4. Carry the user's exact task scope and existing authorization into the worker
   prompt. Delegation does not expand authority or authorize external actions.
   Send the prompt through stdin and request `--output json` for the run result.
5. Inspect the result and verify the relevant diff, artifacts, and checks before
   reporting completion. A successful worker report is evidence to review, not
   proof that the requested work is correct or that external effects occurred.

Keep detailed procedures in the binary's guides. Use native passthrough only
for a provider option needed by this assignment and supported by its CLI.

During the source-install trial, report Due defects or confusing behavior using
the [issue-reporting guide](https://github.com/joshrotenberg/due/blob/main/docs/dogfooding.md).
Preserve the exit status, Due/provider versions, available source commit, and a
minimal sanitized reproduction. Submit an issue when the user's scope authorizes
issue filing; otherwise provide a draft. Inspect partial effects before any
follow-up. Do not include credentials, private task content, or raw provider logs.
