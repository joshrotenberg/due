# due

Due runs one noninteractive Claude Code (`claude -p`) or Codex (`codex exec`)
turn through a single command-line interface. The same options select the
provider, prompt, model, effort, working directory, timeout, and output format
for both. The command prints the final answer, or one versioned JSON result with
status, session ID, usage, and a typed error.

It is meant for scripts and for agents that hand a bounded task to another
agent and need a predictable result to inspect. Each invocation is one
foreground provider run. The caller and operating system own concurrency,
scheduling, and any retry.

```sh
due run --provider claude -p "Explain this repository" --effort high
cat task.txt | due run --provider codex --stdin --output json
due run --request /caller-owned/codex-workspace-profile.json \
  --cwd /canonical/path/to/authorized-checkout -p "Fix the tests"
```

For the last example, save the [workspace profile](examples/codex-workspace-profile.json)
in a protected caller-owned location outside worker writable roots; follow the
[fresh/resume recipe](docs/cli.md#reusable-codex-workspace-profile) for artifacts
and schema transport.

## Install from source

Due is installed from source; there are no published releases. You need:

- Rust 1.90 or newer.
- Read access to the private repository `joshrotenberg/due`.
- The provider CLI you intend to use (`claude` or `codex`), installed and
  authenticated. Due inherits that CLI's environment, settings, and permission
  defaults. It does not install providers or manage credentials.

```sh
git clone https://github.com/joshrotenberg/due.git
cd due
cargo install --path . --locked
due --version
```

`cargo build --release --locked` builds `./target/release/due` without
installing it. The wrapper crates are pinned to Git commits, so the first build
needs access to GitHub and crates.io. `Cargo.lock` is committed and `--locked`
uses it as is.

The binary also carries the skills-only agent plugin. Install it for Claude Code,
Codex, or both without retaining a plugin checkout:

```sh
due init --dry-run
due init                   # Both hosts; use --host claude or --host codex for one
```

Its short `delegate` skill points agents at the installed binary's help and
embedded guides. Due and the selected host CLI must be on PATH. Setup starts no
model work and does not configure authentication or enable the caller trial.
See [installation](docs/install.md) for update, migration and recovery details.

## Quickstart

Check the provider-free parts first. None of these start a provider or use the
network:

```sh
due --help
due run --help
due skills
due run --provider codex -p "Say hello" --dry-run --output json
```

Then run a real turn. Send longer prompts through stdin so they do not appear in
Due's own argv:

```sh
due run --provider claude --stdin --output json <<'PROMPT'
Summarize this repository without changing files. List the paths you read.
PROMPT
```

Text output (the default) prints only the final answer. `--output json` prints
one result document, including for runtime errors. Provider-specific options go
after `--` and are forwarded without a shell:

```sh
due run --provider claude --stdin -- --permission-mode acceptEdits --max-turns 5
```

`acceptEdits` does not approve arbitrary shell commands. For unattended Claude
work, grant the task's edits and checks explicitly and deny unresolved approval
requests; see [unattended permissions](docs/cli.md#unattended-claude-permissions).

Due has no default timeout. Omit `--timeout` while you watch the run and can
cancel it; unattended permissions do not mean nobody is watching. For an
unmonitored run or a real time budget, set a generous, task-specific absolute
limit; progress does not extend it. See
[deadlines and monitoring](docs/cli.md#deadlines-and-monitoring).

```sh
# Explicit time budget for an unmonitored run.
due run --provider codex --stdin --output json --timeout 45m < task.txt
```

## Profiles and dry runs

`--request FILE` reads a versioned JSON request. It can be a complete task or a
partial profile; only `schema_version: 1` is required. Explicit flags override
file values, and omitted flags inherit them. An inherited `timeout` applies
unless you remove it from the file or set it to `null`.

```json
{"schema_version":1,"provider":"codex","effort":"medium","ephemeral":true}
```

```sh
due run --request examples/profile.json --stdin --dry-run --output json < task.txt
due run --request examples/profile.json --provider claude -p "Review the diff"
```

`--dry-run` validates the merged request, native-option conflicts, working
directory, and answer-schema JSON, then reports the executable, argv, cwd, and
stdin size without launching a provider. It does not check authentication, model
support, native CLI option/config compatibility, effective sandbox policy, or
provider schema support. Prompt text is omitted, but native option values and an
inline Claude schema are shown unredacted.

Native arguments after `--` replace the profile's entire `native_args` list,
including permission grants. Changing provider retains inherited policy options;
review them for the selected provider.
Paths in a profile resolve from the caller's directory, not the profile's. There
is no profile search path, inheritance, or config store. Examples are in
[`examples/`](examples).

For persistent Codex work, use
[`examples/codex-workspace-profile.json`](examples/codex-workspace-profile.json).
It uses `-c 'sandbox_mode="workspace-write"'` for both fresh and explicit resumed
runs; Codex CLI 0.162.0 rejects `--sandbox` on `exec resume`. Follow the
[fresh/resume recipe](docs/cli.md#reusable-codex-workspace-profile) to supply each
prompt, cwd, schema, and fresh trace path, then verify the native session ID and
workspace before follow-up. `--ephemeral` conflicts with resume; a later override
cannot recover history that was never persisted. Due adds no session store or
independent sandbox enforcement.

## Contracts, structured answers, and tracing

The binary embeds its own guidance and JSON Schemas, so an agent can inspect the
exact installed contract offline:

```sh
due skills show run            # Scoping, execution, and verifying effects
due skills show contracts      # Request and result semantics
due skills show observe        # Progress traces and trace loss
due schema request             # Also: result, trace, trace-summary
```

`--schema FILE` asks the provider for an answer matching a JSON Schema and
returns it as `structured_output`. The provider enforces its own supported
schema subset. Due does not validate the answer independently; the result
reports `answer_schema_validation: "not_performed"`.

`--trace` prints live progress on stderr; `--trace=json` and `--trace-file PATH`
produce JSONL records with a shared `run_id`. Default records exclude prompts,
argv, paths, model text, reasoning, and tool payloads. `--trace-detail` opts into
bounded labels and assistant messages. Summarize a saved log offline:

```sh
due run --provider codex --stdin --output json \
  --trace --trace-file run.jsonl < task.txt > result.json
due trace summarize run.jsonl --output json
```

| Exit code | Meaning |
| --- | --- |
| `0` | Successful turn or dry run |
| `1` | Provider, protocol, or I/O failure |
| `2` | Invalid invocation, input, or conflicting options |
| `124` | Execution timeout |
| `127` | Provider executable not found |
| `130` | Cancelled by Ctrl-C or Unix SIGTERM/SIGHUP |

## Experimental resident sessions

The opt-in `rpc` feature adds a direct JSON-RPC stdio interface:

```sh
cargo build --locked --features rpc
./target/debug/due rpc
```

It exposes `session.open`, `turn.start`, `session.read`, interruption, interaction
replies and close, with `session.item` streaming. It needs no initialization or
tool envelopes. See the [RPC guide](docs/rpc.md) and
[two-turn JavaScript client](examples/session-harness/rpc-example.mjs).

One session and one active turn run per process. Default builds and `due run`
behavior are unchanged. Set `stream: true` on `session.open` for native items;
`session.read` provides bounded recovery across turns. A
[JavaScript harness example](examples/session-harness/README.md) compares CLI
resume and resident RPC behind one scoped conversation API.

The separate [MCP experiment](https://github.com/joshrotenberg/due/tree/codex/due-mcp-spike)
is preserved on `codex/due-mcp-spike`. This branch contains only the RPC adapter.

## Limits of `due run`

- One foreground turn per invocation. There is no daemon, queue, scheduler, MCP
  server, local session database, or interactive UI.
- No automatic retry after failure, timeout, or cancellation, and no rollback of
  edits a provider made.
- No independent answer-schema validation. Validate `structured_output`
  yourself when its correctness matters.
- No side-effect verification. A successful result and its trace are reported
  evidence; inspect diffs, artifacts, and checks independently.
- Provider permissions, tools, MCP configuration, and budgets stay
  provider-specific. Passthrough validation is not a security boundary.
- Resume IDs are native and provider-specific; `--resume` requires an explicit
  ID.

## Documentation

- [docs/cli.md](docs/cli.md): full CLI reference covering options, request
  fields, structured answers, tracing, native passthrough, results, and errors.
- [Native subagent comparison](docs/native-comparison.md): defaults, existing
  controls, and deferred gaps.
- [docs/install.md](docs/install.md): installation, including the Codex plugin.
- [docs/dogfooding.md](docs/dogfooding.md): trying Due and reporting issues.
- [Caller trial policy](docs/caller-policy.md): opt-in Due-only delegation,
  positive and negative reports, and bounded A/B comparisons.
- [Node caller example](examples/node-caller/README.md): capture one run safely
  and validate a task-specific review answer using Node built-ins.
- [Offline validation example](examples/offline-validation/README.md): optional
  Python caller validation of saved answers, separate from execution and verdict.
- [Native schema smoke example](examples/native-schema-smoke/README.md): explicit
  caller-owned probes with private evidence and provider-free offline tests.
- [Native control audit](docs/native-control-audit.md) and
  [trace evidence](docs/native-trace-evidence.md): versioned observations and limits.
- [Troubleshooting](docs/troubleshooting.md): diagnose installation, host,
  authentication, permissions, and schema failures; repair a frozen source build.
- [CONTRIBUTING.md](CONTRIBUTING.md): development workflow and checks.
- `due run --help` and `due skills show NAME`: the authoritative reference for
  the installed version.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
