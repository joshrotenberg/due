# Recover an existing Due installation

Diagnose the first failing layer before choosing a follow-up. Preserve the failed
result, exit/signal record, stderr, trace, request and partial workspace edits in
private caller-owned storage. A missing terminal trace event is incomplete
evidence, not permission to replay the task. Inspect `git diff` and actual effects
before any deliberate follow-up, which needs a new prompt and fresh artifacts.
Due has no automatic retry, history replay or doctor command.

## Can the calling host start a process?

Use the host's command tool in the same context that launches Due. If the host
rejects execution **before process creation**, neither Due nor the provider has
run. Retain that host diagnostic separately from native task-sandbox failures.

One observed outer-host failure was an unsupported writable root that was a
symlink to a differently named directory. Request a supported host workspace or
permission correction from the human or host administrator; do not
automatically change broad trust settings or add bypass flags. Use the canonical
directory for native workspace grants once host execution is supported:

```sh
cd /path/to/authorized-checkout
pwd -P
```

Correcting a native `--cwd` cannot repair a host that refuses to create processes.

## Is the intended executable present?

In both the terminal and the calling desktop/agent host, inspect:

```sh
command -v due
type -a due
ls -l /path/reported/by/command-v/due
readlink /path/reported/by/command-v/due   # only for a symlink
due --version
due run --help
```

No result from `command -v` suggests missing installation or PATH differences. A
dangling symlink can still exist on disk even when command lookup fails: inspect
the previously configured entry, such as `~/.cargo/bin/due`, and its target.
`cargo clean` or moving a source checkout can remove that target. An unexpected
earlier PATH entry can select an older binary. A binary that exists but cannot
start needs its actual OS/loader/permission diagnostic investigated.

Desktop apps can have a different PATH, home, configured provider home, or OS
session from a terminal. Compare those specific settings privately in the normal
host; do not dump environment variables or authentication files into reports.
Restart a desktop app after an authorized launch-environment change and verify
resolution again from its command tool. A terminal success alone is insufficient.

The source checkout, resolved binary and host plugin cache are separate. Rebuilding
the binary updates embedded `due skills show run` guidance; pulling source alone
does not. A cached plugin entry skill can still be old. Use the existing
[installation/update procedure](install.md) only for the layer requiring repair;
do not edit caches or automatically reinstall plugins or global instructions.

## Does authentication work in the same host context?

Check the exact native executable and installed syntax, without a model call:

```sh
command -v codex
codex --version
codex login --help
codex login status
command -v claude
claude --version
claude auth --help
claude auth status
```

Run the selected provider's status check in the **same normal host context** as
Due, and retain only redacted status and exit code. A restrictive worker sandbox
may block native app-server startup, state-directory access or the OS keychain.
An app-server/state/keychain error, or sandbox-only `Not logged in`, does not
establish that the account login expired. Compare with the authorized normal-host
status before classifying the failure; workers should return the blocker to the
caller rather than widen grants.

If status in the normal host actually reports that login is needed, **the human
must complete the provider's interactive login there**, using installed help.
Then check status in that same context again. Do not use blanket re-login advice
for process, state, permission or network failures. Never copy credential files,
tokens or keychain data into a workspace, another home or evidence bundle.

The [official Codex authentication guide](https://learn.chatgpt.com/docs/auth)
documents `codex login status`, shared cached login state, and file/keyring storage
selected by `cli_auth_credentials_store`. Treat either storage form as secret.
That documentation describes storage and commands; it does not establish which
commands can execute in a particular host sandbox. Due does not manage credentials.

## Did native preparation or task permissions fail?

Keep `due run --help` and the installed provider's `exec --help` /
`exec resume --help` (Codex) or `--help` (Claude) as the syntax reference.
Due's `--dry-run --output json` checks intake and plans argv; it cannot authenticate,
validate native config support or prove effective enforcement. Plans contain
native option values and may contain inline schema data; keep them private.

- **Git preflight:** verify `git -C /canonical/workspace rev-parse --git-dir` in
  an existing authorized checkout. A newly created disposable smoke workspace
  needs authorized Git initialization. Native trust checks still apply; do not
  automatically add `--skip-git-repo-check`.
- **Fresh/resume mismatch:** inspect the planned native argv and installed help.
  The [persistent profile](cli.md#reusable-codex-workspace-profile) uses config-form
  `sandbox_mode` because `--sandbox` is accepted on fresh Codex `exec` but rejected
  on `exec resume` in the versioned evidence. Current caller help checks on
  0.163.0 retain that distinction; the source evidence remains scoped to 0.162.0.
  Resume needs an explicit provider-native session ID and a new prompt; ephemeral
  history cannot be recovered. Verify the native session and workspace.
- **Actual task denial:** distinguish an attempted command's sandbox/approval
  denial from a model declining to attempt it. Check native tool output and
  unchanged targets when available. An expected denial can coexist with successful
  task execution; trace completeness alone does not prove every denial was emitted
  ([versioned trace evidence](native-trace-evidence.md)). Keep inherited restrictions.
  Do not add broad bypass flags, global permission edits or extra writable roots.
- **Temporary files:** when authorized tests need them, create a directory inside
  the worker checkout and set `TMPDIR` only on those test commands. Due's own
  schema snapshots/final-message transport, inputs and artifacts must remain in
  caller-owned paths outside every worker writable root. Do not export the test
  temporary directory to Due itself.

Native arguments after `--` replace the profile's **entire** list, including
permissions. Never silently strip incompatible controls. Preserve the full
intended list in any deliberately revised request; unsupported settings remain
a blocker. Requested/configured values and observed enforcement are distinct.
The [control audit](native-control-audit.md) is versioned source evidence, not a
current effective tool inventory or universal sandbox guarantee.

## Was a schema or connection actually rejected?

For native schema rejection, preserve the original schema bytes, native
diagnostic, versions, result and exit. Due's successful schema JSON preparation
does not establish native dialect/keyword support. A failed execution remains
failed even if a partial structured answer validates. Distinguish explicit null
from missing output using `structured_output_present`; validation, execution and
domain verdict are separate decisions. The optional [offline validator](../examples/offline-validation/README.md)
checks saved answers without a provider call. The [schema smoke example](../examples/native-schema-smoke/README.md)
is an explicitly chosen caller probe, not a retry or schema translator.

Investigate TLS, proxy, DNS or network issues only when actual diagnostics identify
that layer. Record redacted endpoint/error and the failing context, separating
provider model connectivity from sandboxed task-command networking. Do not infer
a TLS problem from `Not logged in` or an absent trace, disable certificate checks,
enable task networking, copy certificates/credentials, or change global proxy
settings as a generic repair. Preserve evidence and request the relevant host or
network correction when confirmed.

## Repair a frozen checkout build through its existing symlink

This is an operator-run repair for an already verified checkout-symlink install.
It does not update source, install a new plugin, change global instructions, or
replace the symlink. First confirm the expected source commit and dirty state,
the existing symlink and its original target directory. Preserve local edits;
stop if the identity differs from the intended frozen source. Record a hash of
any surviving old executable separately from source identity.

```sh
source_root=/absolute/canonical/path/to/existing/due-checkout
target_dir=/absolute/path/to/its/original/target
due_link=/absolute/path/to/existing/bin/due
git -C "$source_root" rev-parse HEAD
git -C "$source_root" status --short
ls -l "$due_link"
readlink "$due_link"
# Confirm these observations match the intended frozen source and original target.
cd "$source_root" || exit 1
cargo build --offline --release --locked --target-dir "$target_dir"
```

Use the supported Rust toolchain (minimum 1.90); record `rustc --version` and
`cargo --version`. This offline command requires cached locked dependencies.
If diagnostics confirm a missing dependency and retrieval is authorized, use
`cargo build --release --locked --target-dir "$target_dir"` instead. Retain
`Cargo.lock`, exact wrapper pins and the default feature set. A network-blocked
worker reports the missing dependency to the caller; it does not expand grants.
Do not pull, reset, clean, use `cargo install --force`, or relink to an experimental
checkout to repair the missing binary.

After a successful build, verify the same existing symlink resolves to the rebuilt
`target_dir/release/due`, inspect its version/help, `skills show run`, `schema request`
and a provider-free dry run using caller-owned inputs/artifacts. Record source
commit/dirty state, resolved executable, build flags/toolchain and binary SHA-256
separately. Identical source does **not** imply identical rebuilt binary hashes.
If the build fails, do not attribute a surviving older executable to that source.

For context, the caller restored frozen main
`76d33d410319d860425a2dd3609863a4ba5ca565` with Rust 1.90, offline locked
dependencies, no added features and the original target directory. The existing
symlink resolved, and version/help/schema/dry-run checks passed. The rebuilt
SHA-256 was `b7c8bc213e943c0c51e0f95cfb838704e1533ee47e3fc130822c6eea0401fd0f`,
different from the historical binary hash. This is a dated caller observation
(2026-10-21), not a required hash for other builds. Approved-host redacted status
checks also succeeded for Codex 0.163.0 and Claude Code 2.1.305; no authentication
files were read or changed. The outer-host symlink-root failure described above
was separate from those results. This maintenance branch adds docs, profiles,
caller examples and fake-provider tests; it does not replace that frozen install.

The caller also rechecked the unchanged eight-setting Codex profile through that
restored executable on 2026-10-21 with Codex 0.163.0. Fresh and explicit resumed
turns shared a native session and returned the original synthetic context token;
each made the intended workspace marker edit and passed two independently rerun
fixture tests. Each fixed probe attempted one outside-canary write, recorded
`EPERM`, and left the canary and probe/test source hashes unchanged. Request
permissions were identical across turns; only the resume ID was added. Both
answers, result envelopes and 21-record traces validated, with no reported loss.
The probe caught its expected denial and exited successfully, so zero failed Due
activities are not evidence of a missing command event. These controls do not
verify effective agent-tool registration, all writable roots, networking or a
universal sandbox. No task replay or permission widening was used.
