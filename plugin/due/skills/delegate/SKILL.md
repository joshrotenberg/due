---
name: delegate
description: Delegate bounded, authorized work (code, documents, research, data) to a separate Claude Code or Codex session through the Due CLI or a configured Due MCP server, and independently verify the result.
---

# Delegate with Due

Due runs a separate Claude Code or Codex session on a bounded assignment and
returns its result. Treat the worker as an independent super-subagent. It works
within the scope you give it, and you review the outcome. The installed
executable's help and guides document the CLI and its available features.

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

Use `due run` for ordinary delegation. For resident work or several workers,
use MCP when the host already exposes tools from a Due MCP server configured
for the assignment's workspace. Check the server's `spawn` schema for the
provider or named profile it accepts, and keep the returned run and session
references for inspection and follow-ups. Confirm that the selected worker
fits the assignment before starting it. If the host has no suitable Due MCP
connection, use the CLI. If an MCP request may have started a worker, resolve
that assignment before starting it again.

For an MCP-enabled build, the installed skills catalog includes the `mcp`
guide. It explains server setup and the tool lifecycle.

Discover what the installed build offers.

```sh
due -h                          # Short help: commands available in this build
due --help                      # Long help: examples and cross-references
due COMMAND --help              # Options for run, script, rpc, skills, ...
due skills list --output json   # Guides installed with this binary
due skills show NAME            # One guide; start with: due skills show overview
```

Read only the guide the task needs. `due skills show overview` helps choose
between a one-shot run and a resident session; `due skills show use-cases`
covers session lifecycle support and patterns such as independent review and
revision. Commands or guides that do not appear in `due -h` or `due skills` are
not part of the installed build; do not assume them or invent flags.

For CLI delegation, `due` must be on PATH. The machine running the worker needs
the selected provider CLI installed and authenticated.

For setup, troubleshooting or reporting a Due problem, read the installed
`overview` guide.
