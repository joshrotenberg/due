# Install from source

Install Due from its public source repository and use its
embedded installer for the skills-only plugin on each machine. No plugin source
checkout is needed after building the executable. The plugin does not install a
binary, provider, or credentials. Normal builds include the resident RPC interface
and embedded JavaScript harness; no Node runtime is needed.

For an existing broken setup, start with [troubleshooting](troubleshooting.md).
It includes an [existing-build repair](troubleshooting.md#repair-a-frozen-checkout-build-through-its-existing-symlink)
that preserves an existing checkout symlink and distinguishes host, login and
task-permission failures.

## Prerequisites

- Rust 1.90 or newer, Cargo, Git, and a native C compiler/linker for embedded JavaScript.
- Claude Code or Codex installed and authenticated locally for whichever provider
  you will run. Check that it works directly before diagnosing Due.
- A plugin-capable local host with permission to execute commands. Work-machine
  repository access, provider availability, and host policy remain local choices.

macOS and Linux are the CI targets. Windows process cleanup and installation have
not been validated. Builds fetch both wrappers and the other locked dependencies
from crates.io. Keep `Cargo.lock` and the wrapper pins unchanged.

## Build and install the CLI

Clone the public repository into a local development directory:

```sh
git clone https://github.com/joshrotenberg/due.git
cd due
git rev-parse HEAD
cargo install --path crates/due-cli --locked
command -v due
due --version
due run --help
```

The Cargo package lives in `crates/due-cli` and installs the `due` executable.
Run the commands below from the workspace root. The default feature set includes the existing `due rpc` and `due script`
commands. Select a smaller build if you only need the foreground CLI or RPC:

```sh
cargo build --locked -p due-cli --no-default-features
cargo build --locked -p due-cli --no-default-features --features rpc
```

The `harness` feature enables `script` and includes `rpc` for its resident
session transport. The foreground `run` command is available in each build.

To include the stdio MCP command and its embedded guide, install with
`cargo install --path crates/due-cli --locked --features mcp-stdio`. Check that the installed
binary exposes `due mcp --help` and `due skills show mcp`, then follow the
[MCP setup guide](../crates/due-cli/embedded-skills/mcp/SKILL.md) to configure your client.

Cargo normally installs under `~/.cargo/bin`. Ensure the host's process environment
can find that directory. A terminal finding `due` does not prove a desktop app has
the same PATH; restart the app after changing its launch environment and check
`command -v due` from an agent's command tool too. A second `due` earlier on PATH
can hide the binary you just installed.

Use the same Git commit on both machines when comparing behavior. During source
dogfooding `due --version` is not a unique build identifier; record the checkout's
commit alongside the resolved binary path.

## Run the release binary from the checkout

For a pull-and-build trial, keep a checkout in a stable development directory
outside temporary or cloud-synchronized storage. A symlink to its release binary
gives normal `due` invocations with no build or wrapper process on startup:

```sh
git clone https://github.com/joshrotenberg/due.git "$HOME/Code/github.com/due"
cd "$HOME/Code/github.com/due"
cargo build --release --locked -p due-cli --target-dir "$PWD/target"
mkdir -p "$HOME/.cargo/bin"
ln -s "$PWD/target/release/due" "$HOME/.cargo/bin/due"
command -v due
due --version
due skills show run
```

The `ln` command fails if an executable or symlink already exists. Inspect and
move that entry to a separate backup before replacing it. Inspect and update an
existing checkout rather than cloning over it. Keep `~/.cargo/bin` on the host's
PATH and verify resolution from the agent itself. Do not use `cargo install` to
update this mode: it can replace the symlink.

From the checkout, preserve local changes, then update:

```sh
git pull --ff-only
cargo build --release --locked -p due-cli --target-dir "$PWD/target"
git rev-parse HEAD
git status --short
due --version
due skills show run
```

Pulling alone does not rebuild Rust or embedded guides. If the build fails, the
previous binary may remain; do not report the pulled commit as the installed
build. Record the successful build's commit and dirty state with trial evidence.
The symlink preserves the caller's working directory. Removing the checkout or
running `cargo clean` makes it unusable until rebuilt. To undo this setup, remove
the symlink and restore the saved executable or use the regular Cargo install.

## Install the bundled plugin

The binary embeds the portable manifest, host compatibility manifests, and
delegate skill through the [`due-plugin` library](../crates/due-plugin/README.md). Once `due` is on PATH, run from any directory:

```sh
due init --dry-run
due init
```

The default is both Claude Code and Codex. Select one if only one host is present:

```sh
due init --host codex --dry-run --output json
due init --host codex
# Or: due init --host claude
```

Both hosts use the same local `due-local` marketplace. Its default root is
`$XDG_DATA_HOME/due`, or `~/.local/share/due` when XDG_DATA_HOME is unset. Override
it with `--install-dir /absolute/path/to/due`. Use `--claude-bin` or `--codex-bin`
for a nonstandard host executable. Use identical options for preview and apply.
`--install-dir` selects package files, not a separate host profile: apply still
registers the plugin in the selected hosts' user configuration. For isolated
acceptance, launch Due with a temporary child-process home and separate native
`CLAUDE_CONFIG_DIR` / `CODEX_HOME` directories, and inspect those hosts' inventories.

Keep the managed directory in place; it replaces the checkout as the plugin's
source. `init` neither copies nor replaces the Due binary, which must remain on
PATH in the host's process environment.

Dry-run reads local filesystem metadata and resolves executables, but writes no
files and starts no host process. It reports missing selected hosts as not ready
with exit 1. It does not prove authentication, native plugin-command support or
host policy. Apply inspects native marketplace/plugin listings, registers the
local marketplace when absent, and installs the delegate skill through the host
CLI. Existing matching marketplace registrations are retained. An existing
Claude user installation is updated; Codex uses its native `plugin add` command.
Conflicting marketplace roots are reported instead of overwritten.

No provider turn, MCP server, background service, permission change or caller
trial is started. Native registration commands have a 30-second deadline and a
64-KiB limit per output stream. Argument vectors are passed directly, without a
shell. Native output received before cancellation or timeout is retained in the failed
step; output-limit failures retain a capped prefix and mark its stream as
truncated. Host output can include local paths; inspect it before sharing.

Start a new host session or reload plugins after installation or an update. The
entry point is `due:delegate`, a short skill that routes agents to `due -h`,
`due --help`, `due COMMAND --help` and `due skills` in the installed binary. Its
guides (`overview`, `run`, `profiles`, `contracts`, `observe`, and `rpc` and
`supervise` where built) cover one-shot work, profiles, raw RPC clients and
supervised assignments without a source checkout or website.
Native host registration succeeding is distinct from verifying skill discovery in
a fresh session. Codex uses the [official local-plugin workflow](https://developers.openai.com/plugins/build/plugins).

### Updating and recovering setup

Rebuild/install Due, then repeat `due init` with the same host and directory
options. Package files come from that exact binary, with no runtime checkout reads
or downloads by Due. Verify the delegate skill in a fresh host session. On a
same-version rebuild, Claude Code 2.1.317 reported the plugin was already up
to date and kept its old cache copy, while a fresh session advertised the
updated skill from the local marketplace. Codex CLI 0.165.0 refreshed its
cache and advertised the update in a new app-server process. Both hosts
advertised the new skill after a version bump. These checks used disposable
host configurations on macOS.

If a fresh session still shows stale content, use the host's plugin manager
to inspect and refresh the installation. Binary-only guide changes do not
require a plugin refresh when the delegate skill is unchanged.

The managed directory is identified by `.due-install.json`. Setup refuses a
nonempty unmanaged directory, symlinked package entries or unexpected files,
so choose an empty directory rather than pointing it at a repository or an
existing host configuration directory. Managed package files are refreshed;
customize a separate copy rather than editing them. A `.due-init.lock` prevents
concurrent apply operations. After an interrupted operation, confirm no setup
process is active before inspecting/removing a stale lock and re-running.

JSON output includes `status`, `plan`, `files_written`, `completed_steps`,
`skipped_steps`, `failed_step` and `error`, with `schema_version` and `cli_version`.
It is a setup report, not a `due run` result. `planned` and `installed` exit 0;
`not_ready` and `partial_install` exit 1. Cancellation exits 130 with status
`cancelled`, after stopping the owned native setup processes. Argument errors
exit 2. A partial installation retains package files and earlier successful host steps. Fix the
reported prerequisite or host error, inspect existing effects, and explicitly
re-run setup. Due does not roll back or automatically retry host operations.

### Migrating a checkout-based installation

`due-dev` remains the legacy source-checkout marketplace. The new managed
installer uses `due-local`; it does not remove an existing `due-dev` registration.
Avoid two active copies by explicitly migrating the old registration. Inspect the current host plugin
list, remove the old Due plugin and marketplace when no longer needed, then run
`due init` and verify only the intended delegate skill is enabled. For Codex:

```sh
codex plugin remove due@due-dev
codex plugin marketplace remove due-dev
due init --host codex
```

Use the Claude plugin manager's equivalent commands for a legacy Claude setup.
Other plugins and host settings are left to the native host. The checked-in
[`plugin/due`](../plugin/due) package remains the canonical skill/manifest source
and can still be used by plugin authors; it is included at compile time.

## Choose how agents use Due

The plugin is a scoped entry point, not a global delegation mandate. It directs
agents to the installed binary's guidance and lets the caller choose `due run`,
`due script`, or an independent RPC client for the authorized assignment.
Installation does not require reports for every run or periodic A/B comparisons.

The earlier source-trial policy is retired. If you previously added a `Due source
trial` block to global `AGENTS.md`, `AGENTS.override.md`, or `CLAUDE.md`, review and
remove that block when you no longer want its separate restrictions. Due does
not edit those files. Preserve unrelated instructions and start a fresh host
session after changing its global guidance. See [caller policy](caller-policy.md)
for the current scope and verification contract.

## Check installation without a provider call

```sh
due -h                         # Short help; --help adds examples
due skills list --output json
due skills show overview
due skills show run
due skills show profiles
due skills show supervise      # Only if `due -h` lists script
due skills show rpc            # Only if `due -h` lists rpc
due script --help
due rpc --help
due schema script-result
due schema request
due run --request examples/profile.json --provider codex \
  -p 'Explain this repository without changing files.' --dry-run --output json
```

Expect `status: planned` for the last command. Dry-run checks the merged task and
local preparation; it does not authenticate a provider or prove model support.
For an authorized live check, use a disposable directory containing a synthetic
text file, ask Due to read it, then check the result and unchanged file yourself.

In a fresh host chat, ask: “Use the Due plugin to inspect this scratch repository
without changing files.” Confirm it discovers `due`, reads the embedded guidance,
uses the chosen provider, and reports independently checked evidence. Installing
the package alone is not a successful behavioral smoke test.

## Update or remove

Choose the update procedure for the binary installation you actually use.
Preserve local changes before updating the checkout.

For a **Cargo-installed binary**, reuse your original `--root` and feature
flags when applicable:

```sh
git pull --ff-only
cargo install --path crates/due-cli --locked --force
due --version
git rev-parse HEAD
```

For a **checkout symlink**, use the pull-and-build procedure in
[checkout installation](#run-the-release-binary-from-the-checkout). Do not run
`cargo install --force` in that mode.

After updating the binary, refresh its managed plugin with `due init` using the
same host and directory options.

### Roll back the binary and plugin

Keep a copy of the working executable before updating. To roll back, restore
that executable or repoint your checkout symlink to the saved build. Check
`command -v due` and `due build-info --output json` from the environment where
your host launches Due.

Run `due init` with the restored executable and the same host and install
directory options. It writes the plugin bundled with that build. Confirm the
restored `due:delegate` skill in a fresh host session. This procedure restored
both the CLI and the skill in disposable Claude Code 2.1.317 and Codex CLI
0.165.0 configurations on macOS.

### Remove the plugin or CLI

To remove the plugin, use the native host commands for your setup:

```sh
codex plugin remove due@due-local
codex plugin marketplace remove due-local
claude plugin uninstall due@due-local --scope user
claude plugin marketplace remove due-local --scope user
```

Run only the commands for hosts you configured. Once neither host registration
uses the managed directory, it can be removed. No `due init` uninstall command or
background cleanup is installed. For a legacy `due-dev` registration, follow the
[migration](#migrating-a-checkout-based-installation) instructions instead.

To remove a Cargo-installed binary, use `cargo uninstall due-cli`. If you
installed with `--root`, pass the same root when uninstalling. For a checkout
symlink, remove the verified symlink and restore the saved executable if desired;
Cargo does not reliably track manually installed symlinks. Do not remove the
checkout until no active runs or plugin registrations use it.

Uninstalling the plugin does not uninstall the CLI.
See [dogfooding and issue reports](dogfooding.md) for what to capture when a step
fails, and the [CLI reference](cli.md) for execution semantics.
