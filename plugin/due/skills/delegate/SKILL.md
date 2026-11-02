---
name: delegate
description: Delegate bounded, authorized coding work to Claude Code or Codex through the installed Due CLI, as an independent super-subagent whose result the caller verifies.
---

# Delegate with Due

Due runs a separate Claude Code or Codex session on a bounded assignment and
returns its result. Treat it as a super-subagent: it works independently inside
the scope you give it, and you review the outcome. This plugin only points to the
installed `due` executable; the executable's help and guides are the source of
truth for that version.

You own the scope and the acceptance. Honor the provider the user chose. State the
workspace, deliverables, constraints and stopping condition, and carry the user's
existing authorization into the task without widening it. Delegation does not
grant external actions or recursive delegation. A worker's success report is
evidence to check against the actual diff, artifacts and tests, not proof.

Discover what this build offers, progressively and only as needed:

```sh
due -h                          # Short help: commands available in this build
due --help                      # Long help: examples and cross-references
due COMMAND --help              # Options for run, script, rpc, skills, ...
due skills list --output json   # Guides installed with this binary
due skills show NAME            # One guide; start with: due skills show overview
```

Read only the guide the task needs. `due skills show overview` helps choose
between a one-shot run and a resident session. Commands or guides that do not
appear in `due -h` or `due skills` are not part of the installed build; do not
assume them or invent flags.

If `due` is not on PATH, report the missing prerequisite. The selected provider
CLI must also be installed and authenticated by the user.

For setup, troubleshooting or reporting a Due problem, read the installed
`overview` guide.
