# Release readiness

Start with the workflows people already use through the CLI and delegate
plugin. Verify them through the installed executable on macOS and Linux before
choosing what to ship in an initial release. Use either Claude Code or Codex on
its own, and cover a mixed-provider workflow where it adds value.

## 1. Establish parity

| Surface | Current baseline | Acceptance before release |
| --- | --- | --- |
| Core | One-shot execution and a resident engine with run and session references, native interaction responses, cursor reads, and settlement. | Exercise the one-shot and resident entry points from a separate Cargo consumer. Cover both providers, stable results and errors, follow-up, interruption, cleanup on disconnect, and reuse of capacity after confirmed release. Keep the scope of any published Rust API a separate decision. |
| MCP | Opt-in stdio server over the core, provider or named-profile selection, multiple live workers, approvals, item pages, and explicit release. | From an installed MCP-enabled binary, discover tools, spawn and follow up, read results and items, respond to approvals, and release a worker. Cover one-provider use and a mixed connection. Verify remaining workers survive release and settle on EOF. Keep profile selection under the server operator's control. |
| CLI | `due run` with prompt or JSON profile, native resume, result and trace output, and offline discovery. The default build also ships RPC and JavaScript. | Verify current flags, versioned JSON, exit statuses, trace behavior, saved-session resume, and build identity. Smoke-test the minimal, default, and MCP-enabled builds. Run the current RPC and script compatibility suites on the build that ships them. |
| Plugin | `due init` previews or manages the delegate skill for Claude Code and Codex, together or individually. The skill uses the CLI by default and can discover an explicitly configured Due MCP server. | Verify preview, install, refresh, partial failure recovery, and fresh-session discovery for each host. Check CLI fallback and the optional MCP route. Use fake hosts in CI and explicitly authorized disposable host configurations for native acceptance. |

Parity means a user can still complete the same workflows and use the same
existing CLI and plugin contracts. The MCP interface can expose a different
sequence of operations. Record which entry points support fresh work, native
resume or fork, and live follow-up. The current MCP follow-up takes a live
session reference, while the CLI can resume with a saved native ID.

Start with an installed-binary acceptance check for the MCP-enabled build.
The default source install smoke and the MCP adapter tests already cover
parts of this separately. Run the packaged binary from outside the checkout
with no provider on PATH. Verify offline help, tool and profile discovery,
and plugin preview, then use fake executables for one resident workflow.
Record the revision, features, platform, and exact commands so later
packaging changes can run the same check.

## 2. Make installation and recovery reproducible

- Give a first-time user one path from the README to a verified install, a
  one-shot request, and plugin discovery. Document the optional MCP setup
  alongside it, including executable resolution from the host, profile
  selection, session lifetime, and cleanup.
- Keep the installed help and skills usable offline. Cross-check them with the
  CLI, MCP tool descriptions, examples, and schemas for the builds that expose
  those interfaces. Keep a short compatibility and limits reference for
  platforms, provider versions tested, and operations that need native support.
- Rehearse update and rollback using disposable directories. Verify build
  identity, managed package ownership, refresh or recovery after partial setup,
  and which host configuration files the operator is responsible for. Treat
  bundling or registering an MCP server as a separate option until its install
  and removal have been validated in both hosts.

## 3. Prepare a release candidate

Decide the initial distribution channel, supported artifact targets, and feature
set from the parity results. A CLI artifact and a published Rust API are
separate decisions. The project currently installs from source and tracks the
public Rust API question in [#95](https://github.com/joshrotenberg/due/issues/95).

The [artifact rehearsal workflow](../.github/workflows/artifact-rehearsal.yml)
builds candidate tarballs for macOS arm64 on macOS 15 and Linux x86-64 on
Ubuntu 22.04. The binary includes the default RPC and JavaScript features
and the stdio MCP adapter. Each tarball carries both licenses and has a
separate SHA-256 checksum. The workflow checks the checksum, unpacks the
tarball, verifies the recorded build identity, and runs offline help from
the extracted executable. GitHub Actions keeps the tarballs as temporary
workflow artifacts.

To rehearse on your machine from a clean checkout, run:

```sh
output_dir=$(mktemp -d)
bash scripts/rehearse-package.sh "$output_dir"
```

Before proposing a release, rehearse the chosen packaging path from a clean
revision and install its artifacts in disposable environments. Verify the
version and source identity, parity checks, documentation links, upgrade and
rollback instructions, license files, and draft release notes. Record the
supported targets and known limits. Publication requires a separate decision.

## Work kept outside this sequence

- The [raw RPC spike](https://github.com/joshrotenberg/due/pull/150) preserves
  ideas for multiplexed subscriptions and cursor replay. A future adapter
  needs to keep approval and interrupt calls available under load and handle
  oversized results without disconnecting unrelated workers. The current
  single-session RPC protocol remains part of the compatibility baseline.
- The [JavaScript](https://github.com/joshrotenberg/due/pull/152) and
  [Lua](https://github.com/joshrotenberg/due/pull/153) adapters are experiments.
  The supported JavaScript harness keeps its current contract during parity work.
- [Named live sessions](https://github.com/joshrotenberg/due/issues/130),
  [host lifecycle adapters](https://github.com/joshrotenberg/due/issues/101),
  and HTTP transport can be evaluated independently of the initial release.
  Keep the [desktop-host report](https://github.com/joshrotenberg/due/issues/146)
  open for a failing matched sample, and treat native observation gaps
  [#6](https://github.com/joshrotenberg/due/issues/6) and
  [#8](https://github.com/joshrotenberg/due/issues/8) as tracking work.
