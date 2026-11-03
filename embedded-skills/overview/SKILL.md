---
name: overview
description: Choose between Due's one-shot run, resident script and raw RPC modes, get started offline, discover installed help and guides, and troubleshoot or report problems.
---

# Due overview

Due delegates a bounded assignment to Claude Code or Codex. The caller states the
scope and decides acceptance; the provider CLI keeps its own authentication,
settings and permissions. Due adds no authorization and does not make the two
providers' permission policies equivalent. A worker's success report is evidence
to check, not proof.

Everything here is read-only guidance shipped inside this binary. It needs no
source checkout, website or provider process.

## Discover what this build offers

Builds can be reduced, so confirm before relying on a command:

```sh
due -h                          # Short help: the commands in this build
due --help                      # Long help: examples and cross-references
due COMMAND --help              # Options for one command (run, script, rpc, init, ...)
due skills                      # Guide names and descriptions
due skills list --output json   # Same catalog for automated selection
due skills show NAME            # One complete guide
due skills show NAME --output json
due schema request              # Also: result, trace, trace-summary, script-result
```

`-h` is the compact reference; `--help` adds examples and detail. A command or
guide missing from `due -h` or `due skills` is not available in this build. In
particular `rpc` and `script` (and the `supervise` and `rpc` guides) depend on how
Due was built. Do not guess flags: read `due COMMAND --help`.

Guides in the full catalog:

| Guide | Read it to |
| --- | --- |
| `overview` | Choose a mode, set up, troubleshoot, report (this guide) |
| `run` | Run one bounded turn, resume it, verify the effects |
| `profiles` | Write a JSON profile and reuse it across run, script and RPC |
| `contracts` | Interpret the result envelope, typed errors and exit codes |
| `observe` | Watch a `due run` through content-limited traces |
| `rpc` | Write a raw stdio JSON-RPC client (when `due rpc` exists) |
| `supervise` | Supervise several turns with embedded JavaScript (when `due script` exists) |

Load only what the task needs.

## Choose a mode

| Need | Use |
| --- | --- |
| One bounded task, one answer | `due run` (`due skills show run`) |
| Follow-up using saved native context, separate processes | `due run --resume ID` |
| Several turns in one worker, live items, permission decisions, interruption, independent verify and revise | `due script` (`due skills show supervise`) |
| Your own program drives a resident session in any language | `due rpc` (`due skills show rpc`) |

Choose the mode that fits the assignment. A resident session supports ongoing
observation and caller-owned follow-up decisions. `due run` is one foreground invocation; the
caller and the OS own concurrency, scheduling and retries. `due script` and
`due rpc` hold one session and one active turn per Due process.

## Get started

Provider-free checks first. None of these starts a provider:

```sh
due --version
due run --help
due run --provider codex -p 'Say hello' --dry-run --output json
```

Expect `status: "planned"`. A dry run validates the merged request, the working
directory and schema JSON. It does not check login, model availability or the
provider's own option support.

Then one real turn. Send the prompt on stdin and ask for the result envelope:

```sh
due run --provider claude --cwd "$PWD" --stdin --output json <<'PROMPT'
Read README.md and summarize the project in three sentences. Do not change files.
PROMPT
```

State the deliverable, working directory, allowed changes and stopping point in
the prompt. After it returns, check the exit status, `status`, and the real
artifacts or diff yourself. Next: `due skills show run` and
`due skills show contracts`.

## Set up

- The provider CLI (`claude` or `codex`) must be installed, authenticated and
  working when run directly on the same host. Due inherits its environment.
- `due init --dry-run` previews installing the bundled agent plugin; `due init`
  applies it (`--host claude` or `--host codex` for one host). It starts no model
  work and changes no permissions or credentials.
- Reusable settings live in JSON profiles you own: `due skills show profiles`.

## Troubleshooting

Preserve the result, stderr, any trace file and partial edits before a follow-up.
Do not retry automatically; failure, timeout and cancellation do not roll back
changes.

| Symptom | First checks |
| --- | --- |
| `due` not found | PATH of the shell that launched the agent; `command -v due` |
| Exit 127 | The provider executable is missing or not on PATH; `--binary PATH` selects one |
| Exit 2, `invalid_input` | Flag conflicts, bad profile JSON, empty prompt; `--dry-run` shows the merge |
| `authentication` error | Run the provider CLI directly in the normal host; a sandbox-only "not logged in" is not proof of expiry. Login is the human's step; never copy credentials |
| Exit 124 | `--timeout` elapsed; it is absolute and not extended by activity |
| Exit 130 | Cancelled by SIGINT, SIGTERM or SIGHUP |
| Task command denied | Native permissions blocked it; report the check as blocked rather than widening grants |
| Resume fails | Wrong provider, wrong workspace, or a session that was ephemeral |

Exit codes for `due run`: 0 success or plan, 1 execution failure, 2 invalid
input, 124 timeout, 127 missing executable, 130 cancellation. `due script` uses
0, 1 and 2 only. See `due skills show contracts`.

## Report a Due problem

When Due itself behaves wrongly or confusingly, capture the command line (without
secrets), exit status, `due --version`, the provider CLI version, and a minimal
sanitized reproduction. Do not include credentials, private task content or raw
provider logs. File an issue only if the user's scope allows it; otherwise hand
over a draft. Inspect partial effects before any follow-up.
