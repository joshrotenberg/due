# Due

Due delegates bounded work to Claude Code or Codex from one command line. Start
a worker for a single assignment, keep a session for follow-up, or supervise
multiple workers over MCP. Use whichever provider you already have installed
and authenticated. Native tools and permissions come from the provider, and the
caller supplies the assignment and verifies the result.

## Install and run

Build from source with Rust 1.90 or newer and a native C toolchain for the
embedded JavaScript runtime.

```sh
git clone https://github.com/joshrotenberg/due.git
cd due
cargo install --path crates/due-cli --locked
due --version
```

Use the provider you have configured.

```sh
due run --provider claude -p "Summarize this repository"
due run --provider codex -p "Summarize this repository"
```

For a script or agent caller, pass the assignment over stdin and request the
versioned JSON result.

```sh
due run --provider codex --stdin --output json < task.txt > result.json
```

Use `due -h` for a command list, `due COMMAND --help` for examples and options,
and `due skills list` for the guides embedded in your build. Run
`due build-info --output json` to record its version, source revision, and
features. See the [installation guide](docs/install.md) for smaller builds,
plugin setup, updates, and rollback.

## Choose an interface

| Interface | When to use it |
| --- | --- |
| `due run` | Run one foreground assignment. Use `--resume ID` to continue saved native history in a later invocation. See the [CLI reference](docs/cli.md). |
| `due mcp` | Give an MCP client multiple live workers, named profiles, follow-ups, session items, and approval handling over stdio. See the [MCP guide](crates/due-cli/embedded-skills/mcp/SKILL.md). |
| `due rpc` | Control a single resident worker from a custom JSON-RPC stdio client. See the [wire reference](docs/rpc.md). |
| `due script` | Supervise resident assignments, permission requests, and independent checks in JavaScript. See the [harness guide](docs/harness.md) and [runnable example](examples/harness/supervision.js). |
| `due init` | Install the bundled `due:delegate` and `due:work-loop` skills in Claude Code, Codex, or both. See the [plugin setup guide](docs/install.md#install-the-bundled-plugin). |

The default build includes RPC and JavaScript. To add the stdio MCP command,
install with `cargo install --path crates/due-cli --locked --features mcp-stdio`,
then configure the MCP client to launch `due mcp --cwd /path/to/workspace`.
The delegate skill uses the CLI by default and can use a Due MCP server
configured by the host operator.

Preview plugin setup with `due init --dry-run`. Select one host with
`due init --host claude` or `due init --host codex`. The installed skill
finds the `due` executable on the host's PATH.

## Profiles, results, and traces

Use `--request FILE` to reuse a versioned JSON profile. Supply the prompt on
each invocation and override profile values with explicit flags as needed.
Use `--dry-run` to inspect the resolved executable, arguments, and workspace.
For saved Codex sessions, start with the
[workspace profile and resume recipe](docs/cli.md#reusable-codex-workspace-profile).

```sh
due run --request examples/profile.json --stdin --dry-run --output json < task.txt
due run --request examples/profile.json --stdin --output json \
  --trace-file run.jsonl < task.txt > result.json
due trace summarize run.jsonl --output json
```

Use `due schema request`, `due schema result`, and `due schema trace` to inspect
the contracts shipped in your executable. Use `--schema FILE` to request
structured output and validate the returned answer in your caller. For
unattended work, give the worker a task-specific time limit and grant only the
provider permissions needed for the assignment. See the
[CLI reference](docs/cli.md) and [caller policy](docs/caller-policy.md).

## Documentation and development

- The [installation](docs/install.md) and [troubleshooting](docs/troubleshooting.md)
  guides cover PATH, native host setup, source builds, and recovery.
- The [delegation design](docs/delegation-interface.md) describes the workspace
  crates, session ownership, and adapter boundaries.
- The [examples](examples) include a Rust consumer, external RPC client,
  JavaScript supervision, and offline result validation.
- The [contributor guide](CONTRIBUTING.md) covers workspace commands and CI.
  The [release readiness plan](docs/release-readiness.md) tracks packaging
  and acceptance checks.
- The [issue reporting guide](docs/dogfooding.md) explains what to capture when a
  provider or Due behaves unexpectedly.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
