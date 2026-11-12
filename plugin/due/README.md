# Due plugin

The delegate skill helps an agent give a bounded assignment to Claude Code or
Codex and verify the outcome. The default path uses the local Due CLI. Start
with `due run` for one turn and use `due skills show overview` to discover other
commands and installed guides.

A host with a configured Due MCP connection can use the same skill for
resident workers and follow-ups. Build Due with `mcp-stdio`, then configure the
host to launch `due mcp --cwd /path/to/workspace`. See `due skills show mcp` in
that build for the tool lifecycle and named profiles. The plugin package
contains the skill. The host operator configures MCP separately.

CLI delegation needs `due` on PATH. The machine running the worker needs an
installed and authenticated provider CLI. Installation starts no model work.
The former source-trial and A/B requirements are retired.

The work-loop skill is for an order larger than one assignment, such as a
backlog of issues or one change with independent review. It is a short entry
that sends the calling agent to `due skills show work-loop`, where the
procedure lives: split the order into pieces, run each piece through a Due
worker in its own workspace, check every result, and keep a board and an inbox
on disk.

Use `due init --dry-run`, then `due init` to install for both hosts, or select
`--host claude` or `--host codex`. Reload plugins or start a fresh host session
after installation. Repeat setup from an updated Due binary to refresh the
managed package. Detailed procedures remain in that binary's help and guides.
