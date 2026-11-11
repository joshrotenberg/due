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
| `release_run` | Free a finished `run` slot and keep the live worker and session journal. |
| `release_session` | Close an idle or ended `session` and free its session and retained run slots. |

## Run and release concurrent workers

Call `spawn` for each independent assignment before waiting for results. Each
call returns its own run and session references. Use the same provider for
several workers or mix providers. A session accepts one active run at a time
and its references are scoped to the connection.

Observe several runs in one call, for example with
`{"runs":["<first run>","<second run>"],"any":true,"timeout_ms":1000}`. Remove
finished runs from the next `any` wait. Check pending interactions with `inspect`
and respond only within your authority. The caller verifies each result against
the work. The connection allows at most 14
positive-duration waits and 15 calls total across those waits, `interrupt`, and
`release_session`. Extra calls return `busy` so a control awaiting settlement
doesn't use the last slot needed by `respond`. Use `timeout_ms: 0` to poll, or
retry a control rejected as `busy` after an admitted call settles.

A `wait` timeout only ends the observation. Use `timeout_ms` on `spawn` or
`follow_up` to set an execution deadline for that assignment. If a run is
waiting for approval, respond with the exact interaction token or interrupt
that run. Submit an authorized response independently if the provider is still
waiting for it during an interrupt. A response receipt confirms acceptance for
delivery; check subsequent items and the run result. If an interrupt returns
`settlement`, use `inspect` to track the run until it finishes. Check the run's
terminal status even if the interrupt call succeeded. Continue monitoring other
workers while one is waiting for input or settlement.

To keep a worker for follow-ups, collect a finished run's result and finish
any waits that include it. Call `release_run` with `{"run":"<run reference>"}`.
Success returns `{"released":true}` and frees a slot in the connection's
retained-run budget. The session reference and journal cursor still work.
An active run returns `busy`. Later lookups and waits still observing a
released run return `unknown_run`.

When done with a worker, wait for it to finish or interrupt it. Collect results
and session items and finish any waits that refer to its runs before calling
`release_session` with `{"session":"<session reference>"}`. Success returns
`{"released":true}` and expires the session and its run references. Other
workers on the connection remain available. A settlement error retains the
records and capacity reservation. A nonzero provider exit can cause this error
even after the provider process exits.

To end the whole stdio connection, close stdin, keep reading stdout through
EOF, and await the Due process. Disconnect starts cleanup for all workers still
owned by the connection. An unsettled worker does not prevent cleanup of its
siblings.

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
