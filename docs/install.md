# Install from source

Due is in a private, source-only dogfooding period. Install the CLI and then the
skills-only plugin on each machine. The plugin does not install a binary, provider,
or credentials. Features are frozen while we collect reliability and usability
issues.

For an existing broken setup, start with [troubleshooting](troubleshooting.md).
It includes a [frozen-build repair](troubleshooting.md#repair-a-frozen-checkout-build-through-its-existing-symlink)
that preserves an existing checkout symlink and distinguishes host, login and
task-permission failures.

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

## Run the release binary from the checkout

For a pull-and-build trial, keep a checkout in a stable development directory
outside temporary or cloud-synchronized storage. A symlink to its release binary
gives normal `due` invocations with no build or wrapper process on startup:

```sh
git clone git@github.com:joshrotenberg/due.git "$HOME/Code/github.com/due"
cd "$HOME/Code/github.com/due"
cargo build --release --locked --target-dir "$PWD/target"
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
cargo build --release --locked --target-dir "$PWD/target"
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

If moving an existing `due-dev` marketplace, inspect `marketplace list`, register
the new checkout with `codex plugin marketplace add`, and verify its resulting
path before reinstalling the plugin. Use the remove/add flow below if required
by the host. A binary update needs no plugin reinstall when its entry skill is
unchanged.

## Enable the caller trial policy

The optional [caller policy](caller-policy.md) covers Due-only delegation,
positive and negative findings, and bounded A/B trials. Installing the plugin
does not enable that global preference automatically.

### Codex

For Codex in every repository, add a pointer to the active global instruction
file. Codex uses `AGENTS.override.md` if present, otherwise `AGENTS.md`, under
its configured Codex home (normally `~/.codex`). Preserve existing instructions
and use the absolute checkout path on that machine:

```markdown
## Due source trial

Use Due exclusively for delegated model work. At the start of each task, read
/ABSOLUTE/PATH/TO/due/docs/caller-policy.md and follow its reporting and A/B
workflow. Direct work by the calling agent remains allowed. If that file or Due
is unavailable, report it and do not substitute another delegation mechanism.
```

Start a fresh chat after changing global instructions. The pointer follows
checkout policy updates on subsequent reads. Host/project instructions can
override global guidance, so verify it with a real task. See
[Codex instruction discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md).

### Claude Code

Merge the following block into `~/.claude/CLAUDE.md`, Claude Code's user-level
instructions for all projects. Preserve existing instructions and replace the
placeholder with the absolute Due checkout path on that machine. You can also
run `/memory`, select the user `CLAUDE.md` entry, and edit it there; opening a
missing file creates it.

```markdown
## Due source trial

At the start of each task, read
/ABSOLUTE/PATH/TO/due/docs/caller-policy.md and follow its workflow.
If the assignment identifies this session as a Due worker, return observations
to the caller. Do not publish reports or initiate A/B trials. Do not recursively
delegate unless the assignment explicitly authorizes it; use Due if authorized.

As the outer caller, use Due exclusively for delegated model work and identify
each recipient as a Due worker in its assignment. Report all favorable and
unfavorable Due findings as sanitized issues or evidence in joshrotenberg/due:
search existing issues first and aggregate routine successes per work batch.
Periodically consider a controlled two-arm comparison, following the policy's
cadence and deferral rules. Use the same prompt, inputs, schema, permissions and
rubric, deliberately varying provider, model or effort. Independently select the
better supported response when one exists and report the outcome; a tie, neither
acceptable, or inconclusive result is valid.

Direct work by the calling agent remains allowed. If Due or the policy file
is unavailable, report the blocker and do not substitute another delegation
mechanism. Explicit task constraints and host permissions still apply.
```

For example, begin worker prompts with: "You are a Due worker. Return your
findings to the caller; do not delegate, publish issues or start A/B trials."
This makes the role explicit even when a worker loads the same user instructions.

Start a fresh Claude Code session and use `/context` to check that the user
`CLAUDE.md` loaded. Then ask it to read the policy and inspect a scratch repository
through Due without changing files. Confirm it finds `due` on PATH, reads
`due --help` and `due skills show run`, and verifies the worker's output.
The Codex plugin installation above does not configure Claude Code; Claude can
use the installed CLI and its embedded guides directly.

These are behavioral instructions, not permission grants. For scoped unattended
permissions, read `due skills show run`. Reconcile conflicting user/project
instructions and verify behavior in a real task. See
[Claude Code instruction files](https://code.claude.com/docs/en/memory).
The pointer reads the checkout policy on subsequent tasks; a new session avoids
relying on previously loaded policy text. No background automation is installed.

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

Choose the update procedure for the binary installation you actually use.
Preserve local changes before updating the checkout.

For a **Cargo-installed binary**:

```sh
git pull --ff-only
cargo install --path . --locked --force
due --version
git rev-parse HEAD
```

For a **checkout symlink**, use the pull-and-build procedure in
[checkout installation](#run-the-release-binary-from-the-checkout). Do not run
`cargo install --force` in that mode.

When the plugin package changes, refresh it separately:

```sh
codex plugin remove due@due-dev
codex plugin add due@due-dev
```

Removing and adding the plugin refreshes its cached package without changing the
installed CLI. Alternatively, reinstall/refresh Due in the desktop Plugins UI if it shows an older
skill, then start a new chat. A Git-backed marketplace uses
`codex plugin marketplace upgrade due-dev` before plugin reinstall; the local
checkout workflow above is the default for this trial. Do not edit plugin caches.

```sh
codex plugin remove due@due-dev
codex plugin marketplace remove due-dev
```

To remove a Cargo-installed binary, use `cargo uninstall due`. For a checkout
symlink, remove the verified symlink and restore the saved executable if desired;
Cargo does not reliably track manually installed symlinks. Do not remove the
checkout until no active runs or plugin registrations use it.

Uninstalling the plugin does not uninstall the CLI.
See [dogfooding and issue reports](dogfooding.md) for what to capture when a step
fails, and the [CLI reference](cli.md) for execution semantics.
