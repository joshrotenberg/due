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

The initial distribution plan is a GitHub Releases tarball for each target.
The project currently installs from source. The public Rust API question is
tracked separately in [#95](https://github.com/joshrotenberg/due/issues/95).

The [artifact rehearsal workflow](../.github/workflows/artifact-rehearsal.yml)
builds once per target and uploads the tarball and SHA-256 checksum as temporary
workflow artifacts. Each smoke job downloads the matching artifact from the same
run, verifies the host, checksum, archive contents, build identity, and features,
then runs offline help and loads the embedded MCP guide with an empty `PATH`.
The binary includes the default RPC and JavaScript features and the stdio MCP
adapter. Both licenses are in the archive.

| Target | Build host | Download smoke tests and planned support |
| --- | --- | --- |
| `aarch64-apple-darwin` | macOS 15 | macOS 15, 26, and 27 on Apple silicon |
| `x86_64-unknown-linux-gnu` | Ubuntu 24.04 LTS | Ubuntu 24.04 LTS and 26.04 LTS on x86-64 |

The support plan assumes current OS updates. The `xcode-27` runner uses the
[macOS 27 preview image](https://github.com/actions/runner-images/blob/main/images/macos/xcode-27-arm64-Readme.md).

To rehearse on your machine from a clean checkout, run:

```sh
output_dir=$(mktemp -d)
bash scripts/rehearse-package.sh "$output_dir"
```

To check a downloaded tarball with its checksum alongside it, pass the target
and the full build revision reported by the workflow.

```sh
bash scripts/smoke-package.sh "$target" "$revision" "$archive"
```

Before proposing a release, rehearse the chosen packaging path from a clean
revision and install its artifacts in disposable environments. Verify the
version and source identity, parity checks, documentation links, upgrade and
rollback instructions, license files, and draft release notes. Record the
supported targets and known limits. Publication requires a separate decision.

### macOS signing and notarization

The artifact rehearsal uses no Apple credentials. A signed candidate needs an
Apple Developer Program team, a Developer ID Application certificate with its
private key available to `codesign`, and Xcode's `notarytool`. For the first
rehearsal, keep the signing identity in a local keychain and authenticate
`notarytool` with a keychain profile. Apple supports a team App Store Connect API
key (`.p8` file, key ID, and issuer ID) or an Apple Account app-specific password
with the team ID. [Developer ID](https://developer.apple.com/developer-id/) and
[Apple's authentication options](https://developer.apple.com/documentation/technotes/tn3147-migrating-to-the-latest-notarization-tool).

1. Sign the staged `due` executable with `codesign --sign "$identity"
   --options runtime --timestamp "$binary"`. Verify it with `codesign --verify
   --strict --verbose=2 "$binary"`.
2. Put that signed executable in a ZIP using `ditto -c -k --keepParent` and
   submit it with `xcrun notarytool submit "$zip" --keychain-profile "$profile"
   --wait`. Require `Accepted` and review the submission log.
3. Package the same signed executable bytes and both licenses in the release
   tarball, then generate its checksum. Apple issues a ticket for the standalone
   executable. Gatekeeper looks it up online because the executable cannot be
   stapled. A stapled DMG or PKG would be a separate choice for offline
   verification. [Apple's notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).
4. On clean Macs or VMs for the supported versions, download the candidate via a
   browser, extract it using the normal user workflow, and run the extracted
   executable with quarantine intact and network access for the ticket lookup.
   Record the Gatekeeper outcome and rerun the package smoke checks against
   those same bytes.

If signing moves to CI, keep it in a separately approved release job. Import
the signing identity into a temporary keychain and keep its private key and the
notary credentials scoped to that job.

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
