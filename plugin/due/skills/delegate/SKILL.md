---
name: delegate
description: Delegate bounded, authorized work (code, documents, research, data) to a separate Claude Code or Codex session through the installed Due CLI, as an independent super-subagent whose result the caller verifies.
---

# Delegate with Due

Due runs a separate Claude Code or Codex session on a bounded assignment and
returns its result. Treat it as a super-subagent: it works independently inside
the scope you give it, and you review the outcome. This plugin only points to the
installed `due` executable; the executable's help and guides are the source of
truth for that version.

You own the scope and the acceptance. Honor the provider the user chose; one
provider is enough, and a fresh session of the same provider can review work it
did not write. State the workspace, deliverables, constraints and stopping
condition, and carry the user's existing authorization into the task without
widening it. Delegation does not grant external actions or recursive delegation.
Workers use their provider's normal settings by default. The calling session's
effective permission mode and temporary approvals do not transfer. Leave native
permission flags unset unless the assignment needs explicit controls. If a
restriction matters, verify it on the actual worker setup as described in
`due skills show run`. Check success against the diff, artifacts and tests.

Prefer doing the work directly when it is small or depends on this conversation's
context. Delegate when it can be written as a self-contained assignment.

Discover what this build offers, progressively and only as needed:

```sh
due -h                          # Short help: commands available in this build
due --help                      # Long help: examples and cross-references
due COMMAND --help              # Options for run, script, rpc, skills, ...
due skills list --output json   # Guides installed with this binary
due skills show NAME            # One guide; start with: due skills show overview
```

Read only the guide the task needs. `due skills show overview` helps choose
between a one-shot run and a resident session; `due skills show use-cases`
matches a task to a pattern such as review and revise, independent assignments
or structured extraction. Commands or guides that do not appear in `due -h` or
`due skills` are not part of the installed build; do not assume them or invent
flags.

If `due` is not on PATH, report the missing prerequisite. The selected provider
CLI must also be installed and authenticated by the user.

For setup, troubleshooting or reporting a Due problem, read the installed
`overview` guide.
