# Due delegate plugin

This skills-only plugin helps an agent run a bounded assignment through the local
Due CLI, using Claude Code or Codex, and independently verify the result and
workspace changes. Choose a one-shot `due run`, a resident JavaScript `due script`,
or an independent raw RPC client. Its short `delegate` skill frames Due as an
independent worker and points to progressive discovery in the installed
executable: `due -h` (short), `due --help` (long), `due COMMAND --help`,
`due skills list --output json` and `due skills show NAME` (start with
`overview`). The executable's help and guides, not this package, document the
commands and contracts of that version.

Keep `due` and the selected provider CLI on PATH. Provider authentication and
permissions remain the host's responsibility. The package contains no MCP server,
hooks or background process. Installing it starts no model work and does not
enable a global delegation/reporting policy. The former source-trial and A/B
requirements are retired. JavaScript is embedded in normal Due builds, so the
harness needs no Node installation.

Use `due init --dry-run`, then `due init` to install for both hosts, or select
`--host claude` or `--host codex`. Reload plugins or start a fresh host session
after installation. Repeat setup from an updated Due binary to refresh the
managed package. Detailed procedures remain in that binary's help and guides.
