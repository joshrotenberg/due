# Due and native subagents

Snapshot: 2026-10-11, Due 0.1.0 source trial, Claude Code 2.1.296 and Codex
0.162.0. Native documentation can describe newer behavior; this is a design
comparison, not a claim that every native feature was exercised in these runs.

Due starts one foreground `claude -p` or `codex exec` invocation. It is a fresh
top-level provider session unless `--resume` is supplied. Native subagents are
children inside a provider's own session. That distinction explains most of the
differences below. Due keeps request/result contracts and process observation
portable; the provider and caller retain their own execution policies.

## Defaults and capabilities

| Area | Native behavior | Due today and assessment |
| --- | --- | --- |
| Deadlines | Claude's SDK documents no total session or per-subagent wall-clock deadline, with a separate output-stall watchdog. [Hosting limitations](https://code.claude.com/docs/en/agent-sdk/hosting#known-limitations) | No execution timeout by default. An explicit `--timeout` is absolute. Keep this default; fix examples that implied a short limit was required. |
| Liveness | Claude documents a 600,000 ms subagent stall timeout; native settings can change it. This is not a total-runtime limit. [Environment variables](https://code.claude.com/docs/en/env-vars) | Opt-in trace with a 5s heartbeat; no idle watchdog. Heartbeats prove Due liveness, not provider progress. Due does not disable native watchdogs. |
| Context | Claude SDK subagents normally start without parent history; a fork receives it. [SDK subagents](https://code.claude.com/docs/en/agent-sdk/subagents) | Due does not transfer the calling agent's conversation. Supply the decision, constraints, relevant paths, expected effects and completion criteria explicitly. Native repository instructions/configuration still apply. |
| Model and effort | Native Codex children normally inherit parent model/effort, subject to spawn, default and custom-agent overrides. [Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents) | Omitted options inherit the Due request profile, then the newly launched CLI's configuration/defaults—not the calling agent's settings. Native effort values outside `low/medium/high` can use passthrough when the shared option is absent. Requested versus observed metadata remains [#1](https://github.com/joshrotenberg/due/issues/1). |
| Permissions | Native Codex children inherit the parent's policy, including live overrides. Claude separates tool availability from permission grants. [Codex subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents), [Claude CLI](https://code.claude.com/docs/en/cli-reference) | A separate CLI does not receive the caller's per-turn grants automatically. Set native permissions for the task. Due validates argument conflicts; it is not a sandbox. Existing unattended examples are the right layer. |
| Nested delegation | Codex enables multi-agent tools by default; an unset concurrency cap uses a Codex-selected default. Claude exposes the Agent tool. [Codex subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents), [Claude subagents](https://code.claude.com/docs/en/sub-agents) | One Due invocation does not guarantee no descendants. Native controls already fit passthrough. Make delegation scope explicit; do not add a scheduler to Due. |
| Workspaces | Claude supports subagent worktree isolation. [Claude subagents](https://code.claude.com/docs/en/sub-agents) | `--cwd` selects a directory; it does not create an isolated checkout. The caller prepares worktrees and separates concurrent writers. This is intentional. |
| Persistence and follow-up | Native clients provide session/thread continuation. [Claude CLI](https://code.claude.com/docs/en/cli-reference), [Codex CLI](https://learn.chatgpt.com/docs/cli/reference) | `--resume ID` already continues a top-level native session with a new prompt. Persistence is allowed by default; `--ephemeral` opts out. Example profiles choosing ephemeral mode are not binary defaults. Keep the ID and native session storage when follow-up is expected. |
| Midstream correction | Codex App Server exposes steering of an active turn. [App Server](https://learn.chatgpt.com/docs/app-server) | Due has cancellation, but no live input channel after prompt intake. Resume covers later refinement; steering remains a separate deferred case. No server is required by the current design. |
| Contracts and effects | Both non-interactive CLIs support structured output. [Claude CLI](https://code.claude.com/docs/en/cli-reference), [Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode) | Versioned input/result, dry-run, optional answer schema and trace schemas are useful additions. Native success, independent answer validation and verified effects remain separate. Independent validation is [#5](https://github.com/joshrotenberg/due/issues/5); native schema compatibility is [#7](https://github.com/joshrotenberg/due/issues/7). |
| Progress and accounting | Native SDK/session protocols expose more provider-specific lifecycle data than a common CLI envelope. [Claude SDK subagents](https://code.claude.com/docs/en/agent-sdk/subagents), [Codex App Server](https://learn.chatgpt.com/docs/app-server) | Due reports observed activities and available usage, but has no normalized child-agent tree or guaranteed descendant cost accounting. Missing native events are still possible ([#6](https://github.com/joshrotenberg/due/issues/6)). Child attribution is deferred in [#8](https://github.com/joshrotenberg/due/issues/8). |
| Budgets and extensions | Claude has native turn and dollar-budget options, plus tool/configuration controls. [CLI reference](https://code.claude.com/docs/en/cli-reference) | Use native passthrough for these controls. A wall-clock limit is not a dollar cap. Skills, hooks, memory and MCP configuration remain native; Due being CLI-only does not disable provider integrations. |

## Existing controls to use deliberately

For a task that must not use the providers' built-in subagent tools:

```sh
due run --provider codex --stdin --output json \
  -- -c agents.enabled=false -c features.multi_agent_v2=false < task.txt
due run --provider claude --stdin --output json \
  -- --disallowedTools Agent < task.txt
```

Codex 0.162.0 resolves an enabled `features.multi_agent_v2` before
`agents.enabled=false`; disable both for this example. The `multi_agent` feature
listing may remain true with `agents.enabled=false`: it reports feature values,
not the final per-model tool gate. See the [versioned control audit](native-control-audit.md).
These narrow native delegation tools, not all possible subprocess creation.
Combine them with the task's actual sandbox/tool permissions. They are opt-in
examples, not new Due defaults. When using a request profile, arguments after
`--` replace its **entire** `native_args` list; keep any required permission
arguments in the replacement.

Use a self-contained prompt for a fresh run. For a later refinement, save the
result's native `session_id`, avoid `--ephemeral`, and use `--resume` with the
same provider and available native session storage. Turning off ephemeral mode
later cannot recover a session that was never persisted. A native child-agent
ID is not interchangeable with Due's top-level session ID.

## Decisions during the feature freeze

Update guidance and examples now: monitored runs without arbitrary deadlines,
explicit context handoff, native permission/configuration inheritance, delegation
scope and persistence choices. See [deadlines and monitoring](cli.md#deadlines-and-monitoring).

Prioritize evidence before adding controls: requested/observed configuration
([#1](https://github.com/joshrotenberg/due/issues/1)), reproducible build identity
([#2](https://github.com/joshrotenberg/due/issues/2)), and answer validation
([#5](https://github.com/joshrotenberg/due/issues/5)). Investigate native child
attribution ([#8](https://github.com/joshrotenberg/due/issues/8)) without promising
a complete activity ledger.

Keep queues, concurrency management, retries, worktree creation and session
storage outside Due. Record real cases for [midstream correction](dogfooding.md)
before designing a transport. These are backlog assessments, not approved
runtime additions.
