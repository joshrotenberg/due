# Delegation design

Due delegates an assignment to a Claude Code or Codex worker. A caller can use
either provider on its own or choose a provider for each worker. The wrappers
run the native CLIs, and the providers continue to execute their own tools.
Due shares lifecycle operations across providers while preserving
provider-specific tool policy, approval responses, and session items.

The [core API guide](core-spike.md) documents the implementation in `due-core`,
including examples, retention bounds, and cleanup guarantees.

## One owner for a worker's lifetime

A core engine owns the workers it starts. Workers in separate sessions can run
concurrently. Each session accepts one active assignment at a time. A finished
run leaves its resident session available for follow-up. Explicit shutdown
settles active work, closes idle workers, and reports cleanup failures.

The entry point determines the owner's lifetime. The [foreground CLI](cli.md)
presents one assignment and one result, with native resume available across
invocations. [Raw RPC](rpc.md) gives a caller control
of a resident session. The [JavaScript harness](harness.md) supervises a workflow
with streamed items, permission replies, and independent verification. Direct
library callers can own a multi-worker engine inside their application.

## Run references and session references

A run reference identifies one admitted assignment. A session reference
identifies the live conversation and its resolved provider configuration.
Both are scoped to their owning engine. A caller can inspect a retained run
result repeatedly and use the same live session for a later assignment.

A saved session descriptor carries a native resume ID and explicit configuration.
It can restore persisted conversation context in a new native process. Effective
permissions and temporary grants are resolved by the new invocation. Ephemeral
work can continue in its live session while the owner is active.

## The caller owns orchestration

Due validates worker configuration, admits assignments, retains results, and
provides exact-run interruption. Waiting observes completion. A wait deadline
leaves the assignment running. The assignment deadline and explicit interruption
have their own settlement rules.

Callers choose providers, decide when to follow up, handle surfaced interactions
within their authorization, and verify results against task effects. A successful
turn records the provider's outcome. Acceptance belongs to the caller.
[JSON profiles](cli.md#json-requests-and-profiles) carry reusable parameters.
The installed guides and plugin explain how to use them.

## Library and CLI boundaries

`due-core` owns provider execution, typed worker options, session and run
state, approvals, session items, and cleanup. The adapters depend on this
library. Each exposes the operations and lifecycle its interface can support.

| Layer | Boundary |
| --- | --- |
| `due-cli` | Ships the `due` executable. The base build uses `due-core/oneshot` for `due run` and includes offline help and guides. Optional features select the adapters and commands. |
| `due-mcp` | A reusable library that maps MCP tools and schemas onto the core. The CLI can expose its stdio server with an opt-in `mcp-stdio` feature. |
| `due-rpc` | A proposed reusable JSON-RPC library with caller-supplied I/O. A future extraction starts with the shipped `due rpc` protocol and its compatibility tests. |
| `due-harness` | The embedded language runtime and workflow API. Today JavaScript launches a Due RPC child. A future in-process core backend could make the CLI's harness feature independent of RPC. |
| `due-plugin` | Proposed library for package assets and installation. The CLI retains the `due init` command and its output contract. |

The default CLI build includes RPC and JavaScript. A build with no default
features retains the one-shot command and offline discovery.

Library entry points take explicit configuration and return results to the
embedding application. The executable owns flags, process signals, exit codes,
and local setup. A separate Cargo consumer should exercise each library entry
point before Due commits to a published API. Track that commitment in
[#95](https://github.com/joshrotenberg/due/issues/95).

## MCP stdio first

The [stdio adapter](https://github.com/joshrotenberg/due/pull/165) and
[named profiles](https://github.com/joshrotenberg/due/pull/166) are on main.
The library accepts host-supplied streams and configuration. The opt-in
`due mcp` command supplies the workspace, stdio, and shutdown signal.

One stdio connection owns one engine with multiple workers. Clients can start
a worker, wait or inspect by run reference, follow up by session reference,
read session items by cursor, and answer a pending approval. EOF or a transport
failure starts cleanup for that connection's workers. The host should receive
the cleanup outcome.

Verify the library from a separate Cargo consumer and exercise the CLI feature
through a real stdio connection with fake providers. Cover both providers,
including follow-ups, approvals, item reads, and cleanup on disconnect. Verify
the base CLI build and the opt-in MCP build independently.

Consider HTTP as a separate transport feature after stdio. HTTP needs explicit
decisions about client identity, session ownership across requests, and
disconnect or expiry behavior. RPC extraction, harness changes, and a packaged
external JavaScript client follow separate decisions. Detached services and
live session registration remain [separate experiments](https://github.com/joshrotenberg/due/issues/130).

## Plugin crate and host registration

Extract the current package assets, host manifest rendering, install plan, and
managed installation into `due-plugin`. Keep Clap arguments, terminal output,
and the process exit code in `due-cli`. Pass the Due executable path and a
shutdown signal to the library. Keep the existing managed tree, locking, and
setup receipt compatible with installed packages.

| Configuration | Behavior |
| --- | --- |
| Default `due-cli/plugin` and `due-plugin/cli` | `due init` installs the delegate skill. Ordinary assignments use `due run`. |
| Minimal CLI with the proposed `plugin` feature disabled | `due run`, help, and installed guides are available. |
| CLI with `mcp-stdio` | `due mcp` is available for an explicitly configured host. |
| Proposed optional `due-plugin/mcp` | Adds MCP package rendering alongside the delegate skill for a host and workspace the operator selects. A Due binary that packages its own server also needs `mcp-stdio`. |

The first extraction should move the current CLI package and installer
together. Keep the plugin crate independent of worker execution. Prove the
same package files, dry-run plan, and fake-host registration results before
adding an MCP packaging option.

Both installed CLIs advertise direct stdio registration through `mcp add`.
Claude Code exposes local, project, and user scopes. It also discovers
plugin-root `.mcp.json` or entries in its plugin manifest. Codex supports
direct registration in its MCP configuration and has portable and compatibility
plugin formats with distinct MCP configuration files. Treat direct
registration and plugin packaging as separate installation paths.
[Claude Code MCP](https://code.claude.com/docs/en/mcp) ·
[Claude Code plugin manifests](https://code.claude.com/docs/en/plugins-reference#mcpservers) ·
[OpenAI Docs for Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) ·
[OpenAI Docs for plugin packaging](https://developers.openai.com/plugins/build/plugins)

Before enabling MCP packaging, test one fixture package per host in a disposable
configuration. Verify manifest discovery, the actual launch arguments and
workspace, duplicate-name handling, and removal. Check which configuration
resident Claude and Codex workers inherit so a host-level Due server does not
accidentally become a recursive delegation route. The observed evidence for
this design is the documentation and local help for Claude Code 2.1.317 and
Codex CLI 0.165.0. Host registration and plugin loading still need validation.
