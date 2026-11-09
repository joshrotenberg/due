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

## Parked proposals

The [MCP adapter experiment](https://github.com/joshrotenberg/due/pull/149) and
an external JavaScript client over MCP are parked. The associated proposal to
retire raw RPC and the embedded JavaScript harness is also parked. Both
`due rpc` and `due script` remain supported.

A detached service, hot session registry, and cross-client attachment remain
[separate experiments](https://github.com/joshrotenberg/due/issues/130).
