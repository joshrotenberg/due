---
name: mcp
description: Configure a stdio MCP client to start Due workers, follow up, read session items, and reply to approvals.
---

# Due over MCP

An MCP-enabled Due build includes the `mcp` command. Build it from a source
checkout with `cargo build --locked -p due-cli --features mcp-stdio`. Configure
your MCP client to start that executable with these arguments.

```text
mcp --cwd /path/to/authorized/workspace
```

Use `due mcp --help` to check the command. Each connection owns a set of Claude
or Codex workers. Choose either installed and authenticated provider for a worker.
The workspace is set by the server operator. The provider CLI resolves its own
settings and permissions. Plugin users can use the delegate skill with a Due
connection configured by the host operator. For an ordinary one-turn task, use
`due run`.

| Tool | Use |
| --- | --- |
| `spawn` | Start a fresh worker with `prompt` and the selector in the tool schema. An optional `timeout_ms` sets the assignment deadline. |
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

## Named worker profiles

The server operator can launch Due with
`due mcp --cwd /path/to/workspace --profiles /path/to/profiles.json`.
The file is loaded and validated before the server accepts MCP requests.

```json
{
  "schema_version": 1,
  "profiles": [
    {"name": "author", "provider": "claude"},
    {"name": "reviewer", "provider": "codex", "interaction_mode": "reject"}
  ]
}
```

Profile entries can also set `model`, `effort`, `ephemeral`, and `native_args`.
Native arguments use the core's resident option validation. Interaction mode
defaults to `manual`. Use `reject` to stop a worker that requests an interaction.
The workspace and provider executable overrides come from server configuration.

With a catalog, `tools/list` advertises the available profile names on
`spawn`. For example, pass `{"profile":"reviewer","prompt":"Review the diff."}`.
Without a catalog, pass `{"provider":"codex","prompt":"Review the diff."}`.
Follow-ups use the original session and its selected settings.
