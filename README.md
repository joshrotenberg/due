# due

Due delegates bounded work to Claude Code or Codex through one command-line
interface. Use `due run` for one noninteractive turn, `due script` for a
supervised JavaScript assignment spanning several turns, or `due rpc` from your
own client. Native authentication, settings and permissions stay with the
provider; the caller supplies the assignment and independently checks its result.

The embedded JavaScript harness keeps one worker session alive while the caller
observes items, answers permission requests, runs independent checks, and decides
whether another turn is needed. Rust owns process limits, cancellation and cleanup.
No Node installation is needed. Due does not add a background daemon or choose
acceptance criteria for the caller.

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

## What it is for

Due is useful with only one provider installed. A worker is a separate session
with its own context, so a fresh session of the same provider can take a narrow
brief, hold a large body of material across follow-up turns, or review work it
did not write. Running the other provider as well is optional. Common patterns:

- Bounded delegation: one assignment with a deliverable and a stopping point.
- Context-heavy specialist: follow-ups through `--resume ID` or a resident session.
- Review, verify, revise: a fresh worker critiques, the caller runs the checks,
  the author revises.
- Independent assignments: separate `due run` processes that the caller starts
  and collects. Due has no scheduler or pool and does not isolate workers.
- Structured extraction or transformation: `--schema` answers the caller validates.
- Embedding in a caller: `--output json` and exit codes from a script or
  program, or the stdio RPC protocol.

Code, documents, research and data are all inputs to the same patterns. For
small tasks, or for live back-and-forth with a person steering, working directly
or in the provider's interactive CLI is simpler. `due skills show use-cases`
covers each pattern and when to choose something else.

## Install from source

Due is installed from source; there are no published releases. You need:

- Rust 1.90 or newer and a native C compiler/linker for embedded JavaScript.
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
installing it. The first build fetches both wrappers and the other dependencies
from crates.io. `Cargo.lock` is committed and `--locked` uses it as is.

`due build-info --output json` reports the installed executable's package
version, source revision, dirty/unknown state and enabled features. It reads no
checkout or Git state at runtime. Source archives and builds without Git report
unknown source metadata. The build-time probe deliberately runs on every Cargo
build of Due to avoid stale identity, including after failed builds. This
recompiles Due's own targets on otherwise unchanged builds; dependencies can
remain cached. `due --version` stays compact. Build identity is a diagnostic
snapshot, not a reproducible-build attestation.

The binary also carries the skills-only agent plugin. Install it for Claude Code,
Codex, or both without retaining a plugin checkout:

```sh
due init --dry-run
due init                   # Both hosts; use --host claude or --host codex for one
```

Its short `delegate` skill frames Due as an independent worker and points agents
at the installed binary's `due -h`, `due --help` and `due skills` for the rest. Due and the selected host CLI must be on PATH. Setup starts no
model work and does not configure authentication or enable a global delegation policy.
See [installation](docs/install.md) for update, migration and recovery details.

## Quickstart

Check the provider-free parts first. None of these start a provider or use the
network. Help comes in two lengths: `-h` is the short command and option list,
and `--help` adds examples and detail. Every subcommand has both.

```sh
due -h                       # Short: the commands in this build
due --help                   # Long: examples and where to read next
due run --help               # Full option reference for one command
due skills                   # Guides embedded in this binary
due skills show overview     # Choose a mode, get started, troubleshoot
due run --provider codex -p "Say hello" --dry-run --output json
```

Reduced builds may omit `script` or `rpc` (and their guides); `due -h` and
`due skills` show what the installed binary offers.

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

`due run` has no default timeout. Omit `--timeout` while you watch the run and can
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
due skills list --output json  # Catalog for automated selection
due skills show overview       # Choose a mode, set up, troubleshoot, report
due skills show use-cases      # Whether Due fits a task, and which pattern
due skills show run            # Scoping, execution, and verifying effects
due skills show profiles       # JSON profiles for run, script and RPC
due skills show contracts      # Request and result semantics
due skills show observe        # Progress traces and trace loss
due skills show rpc            # Raw stdio client wire reference (RPC builds)
due skills show supervise      # Resident assignments and JavaScript supervision
due schema script-result       # The JavaScript harness result envelope
due schema request             # Also: result, trace, trace-summary
```

Installed guides are complete without a source checkout or website; the links in
this repository are supplemental.

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

## Supervised assignments and raw RPC

Normal builds include the JavaScript harness and JSON-RPC stdio interface:

```sh
due script examples/harness/supervision.js \
  --cwd /path/to/authorized/project --evidence /tmp/due-assignment-001 \
  --args-file /caller-owned/assignment.json > result.json
```

The [JavaScript guide](docs/harness.md) describes the supported `Due.run` API,
JSON arguments and results, permission callbacks, command bounds, and evidence.
The [runnable example](examples/harness/supervision.js) keeps one session across
implementation, independent verification and bounded revision. The caller selects
the verifier and checks its result; a model's claim that tests passed is not
acceptance. Existing versioned JSON profiles work in `scope.open({request: ...})`.

For a custom client, launch `due rpc` and send newline-delimited JSON-RPC.
It exposes `session.open`, `turn.start`, `session.read`, interruption, interaction
replies and close, with `session.item` streaming. One session and one active turn
run per process. The raw protocol is independent of the embedded harness: callers
can implement it in any language. See the [RPC guide](docs/rpc.md) and
[two-turn external JavaScript client](examples/session-harness/rpc-example.mjs).

Use `due run --resume ID` when separate invocations and native saved context are
sufficient. Use a resident session when you need continuing observation, explicit
permission replies, or interruption across an assignment. The
[language comparison](docs/rpc-languages.md) records why JavaScript was selected;
Lua remains a comparison fixture, not a shipped harness language. The separate
[MCP experiment](https://github.com/joshrotenberg/due/tree/codex/due-mcp-spike)
remains outside this implementation.

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

- [Core library spike](docs/core-spike.md): experimental multi-provider Rust
  interface, concurrent native workers, ownership and fake-provider validation.
- [docs/cli.md](docs/cli.md): full CLI reference covering options, request
  fields, structured answers, tracing, native passthrough, results, and errors.
- [JavaScript harness](docs/harness.md): supported session supervision, JSON results and evidence.
- [Raw RPC](docs/rpc.md): wire contract for independently written clients.
- [Native subagent comparison](docs/native-comparison.md): defaults, existing
  controls, and deferred gaps.
- [docs/install.md](docs/install.md): installation, including the Codex plugin.
- [docs/dogfooding.md](docs/dogfooding.md): trying Due and reporting issues.
- [Caller policy](docs/caller-policy.md): scope, verification and optional issue reporting;
  the former mandatory source-trial workflow is retired.
- [Node caller example](examples/node-caller/README.md): capture one run safely
  and validate a task-specific review answer using Node built-ins.
- [Offline validation example](examples/offline-validation/README.md): optional
  Python caller validation of saved answers, separate from execution and verdict.
- [Native schema smoke example](examples/native-schema-smoke/README.md): explicit
  caller-owned probes with private evidence and provider-free offline tests.
- [Native control audit](docs/native-control-audit.md) and
  [trace evidence](docs/native-trace-evidence.md): versioned observations and limits.
- [Troubleshooting](docs/troubleshooting.md): diagnose installation, host,
  authentication, permissions, and schema failures; repair an existing source build without changing its revision.
- [CONTRIBUTING.md](CONTRIBUTING.md): development workflow and checks.
- `due -h`, `due COMMAND --help` and `due skills show NAME`: the authoritative
  reference for the installed version, available offline.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
