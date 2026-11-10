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
| Plugin | The CLI can gate `due init` and bundled installer assets with a default-on feature. Installed guides remain available in the base build. |

The default CLI build includes RPC and JavaScript. A build with no default
features retains the one-shot command and offline discovery.

Library entry points take explicit configuration and return results to the
embedding application. The executable owns flags, process signals, exit codes,
and local setup. A separate Cargo consumer should exercise each library entry
point before Due commits to a published API. Track that commitment in
[#95](https://github.com/joshrotenberg/due/issues/95).

## MCP stdio first

Advance the [MCP adapter](https://github.com/joshrotenberg/due/pull/149) first.
Reuse its tool schemas, core operation mapping, and stdio fixtures on current
main. The current core already supplies cursor reads and typed worker
validation. Move configuration and tool construction into the library. Use a small
CLI entry point to supply the workspace, provider configuration, stdio, and
shutdown signal. Evaluate the [validated profile catalog](https://github.com/joshrotenberg/due/pull/155)
as host-supplied configuration for this adapter.

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
