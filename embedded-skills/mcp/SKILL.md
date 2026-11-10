---
name: mcp
description: Configure a stdio MCP client to start Due workers, follow up, read session items, and reply to approvals.
---

# Due over MCP

Configure your MCP client to start the `due` executable with these arguments:

```text
mcp --cwd /path/to/authorized/workspace
```

Use `due mcp --help` to check the command. Each connection owns a set of Claude
or Codex workers. Choose either installed and authenticated provider for a worker.
The workspace is set by the server operator. The provider CLI resolves its own
settings and permissions.

| Tool | Use |
| --- | --- |
| `spawn` | Start a fresh worker with `provider` and `prompt`. An optional `timeout_ms` sets the assignment deadline. |
| `inspect` | Read the snapshot for a `run`, including terminal results and pending interactions. |
| `wait` | Observe one or more `runs`. Use `any: true` to wait for the first to finish, or `timeout_ms: 0` to poll. A wait can return before the runs finish. |
| `follow_up` | Send another `prompt` to an idle live `session`. This returns a new run reference. |
| `read_items` | Read a page of session items. Start with `after: 0` and pass back the returned cursor. |
| `respond` | Submit a provider-native `response` object for an exact pending `interaction`. |
| `interrupt` | Interrupt the specified active `run` and wait for its outcome. |

Save the run and session references from `spawn`. A session reference is scoped
to the connection. Use separate sessions for independent author and reviewer
work. The caller checks the actual work before accepting it. If a worker needs
approval, inspect the pending request and respond only within your authority.
Closing the connection settles active assignments and closes idle workers.
