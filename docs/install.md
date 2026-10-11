# Install from source

Due is in a private, source-only dogfooding period. Install the CLI and then the
skills-only plugin on each machine. The plugin does not install a binary, provider,
or credentials. Features are frozen while we collect reliability and usability
issues.

## Prerequisites

- Access to [joshrotenberg/due](https://github.com/joshrotenberg/due) from that machine.
- Rust 1.90 or newer, Cargo, Git, and a native linker/toolchain.
- Claude Code or Codex installed and authenticated locally for whichever provider
  you will run. Check that it works directly before diagnosing Due.
- A plugin-capable local host with permission to execute commands. Work-machine
  repository access, provider availability, and host policy remain local choices.

macOS and Linux are the CI targets. Windows process cleanup and installation have
not been validated. Builds fetch the exact Git-pinned wrapper revisions plus
locked crates; keep `Cargo.lock` and the wrapper pins unchanged.

## Build and install the CLI

Clone into a normal local development directory with GitHub SSH access, or use
`gh repo clone joshrotenberg/due` with your authenticated GitHub CLI:

```sh
git clone git@github.com:joshrotenberg/due.git
cd due
git rev-parse HEAD
cargo install --path . --locked
command -v due
due --version
due run --help
```

Cargo normally installs under `~/.cargo/bin`. Ensure the host's process environment
can find that directory. A terminal finding `due` does not prove a desktop app has
the same PATH; restart the app after changing its launch environment and check
`command -v due` from an agent's command tool too. A second `due` earlier on PATH
can hide the binary you just installed.

Use the same Git commit on both machines when comparing behavior. During source
dogfooding `due --version` is not a unique build identifier; record the checkout's
commit alongside the resolved binary path.

## Install the plugin in Codex

From the checkout root:

```sh
codex plugin marketplace add "$PWD"
codex plugin marketplace list
codex plugin add due@due-dev
codex plugin list --marketplace due-dev
```

If your CLI lacks `plugin add`, register the marketplace, restart the desktop app,
choose **Due (source)** in Plugins, and install **Due** there. Start a new chat to
load its skill. The packaged entry point is `due:delegate`; it routes to help and
the three skills embedded in the installed binary. This setup follows the
[OpenAI local-plugin workflow](https://developers.openai.com/plugins/build/plugins).

Keep this checkout in place: the local marketplace points to it. Hosts can cache
an installed plugin; edits in the checkout alone do not prove the cached skill
was refreshed. The package follows [Agent Plugins](https://agent-plugins.org/plugin-authors/build-an-agent-plugin);
other clients need their own supported installation flow.

## Check installation without a provider call

```sh
due skills list --output json
due skills show run
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

From your checkout, preserve local changes before updating:

```sh
git pull --ff-only
cargo install --path . --locked --force
codex plugin remove due@due-dev
codex plugin add due@due-dev
due --version
git rev-parse HEAD
```

Removing and adding the plugin refreshes its cached package without changing the
installed CLI. Alternatively, reinstall/refresh Due in the desktop Plugins UI if it shows an older
skill, then start a new chat. A Git-backed marketplace uses
`codex plugin marketplace upgrade due-dev` before plugin reinstall; the local
checkout workflow above is the default for this trial. Do not edit plugin caches.

```sh
codex plugin remove due@due-dev
codex plugin marketplace remove due-dev
cargo uninstall due
```

These are separate removals: uninstalling the plugin does not uninstall the CLI.
See [dogfooding and issue reports](dogfooding.md) for what to capture when a step
fails, and the [CLI reference](cli.md) for execution semantics.
