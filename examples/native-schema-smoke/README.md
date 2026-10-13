# Optional native schema smoke

This caller-owned example prepares backlog #7 probes using installed Due and the
existing [Node caller](../node-caller/README.md). It is not a Due command or a
compatibility guarantee. **No live native compatibility is established by the
offline tests.** The caller owns authorization, native authentication, effective
permissions, model selection, observation, and interpretation of live evidence.

The harness requires Python 3.11+, the existing `jsonschema` dependency described
in [offline validation](../offline-validation/README.md), and Node.js 22+.
Offline testing used Python 3.13.0, `jsonschema` 4.26.0, `referencing` 0.37.0,
and Node.js 26.11.1. Nothing installs dependencies, edits native configuration,
or changes global permissions. Supply the installed Due executable explicitly.

## Choose one case

| `--case` | Original schema | What to observe |
| --- | --- | --- |
| `minimal` | Required `{summary: string}`, no extra properties, no `$schema`. | Common baseline; local validation uses Draft 2020-12. |
| `dialect` | Same contract with explicit Draft 2020-12 `$schema`. | Candidate native dialect-declaration rejection. |
| `keyword` | Baseline plus `not: {const: ""}` on `summary`. | Candidate unsupported `not` keyword; local validation rejects an empty summary. |

The two negative candidates are valid local Draft 2020-12 schemas. Their native
acceptance/rejection is **version-specific**, not asserted in advance. The
[Node example's historical observation](../node-caller/README.md) records Claude
Code 2.1.296 rejecting an explicit Draft 2020-12 declaration; it does not predict
other versions. The caller chooses which candidate merits a live invocation.
There is no matrix runner, automatic comparison, fallback, or retry.

Original schema bytes are copied unchanged and hashed before launch. Nothing
strips a dialect declaration, removes an unsupported keyword, or translates a
schema. The prompt asks only for `{"summary":"schema smoke"}` and explicitly
forbids repository inspection, tool use, writes, networking, recursive workers,
repair, retries, and publication. Native repository instructions/settings may
still be loaded by the provider; use an authorized, disposable workspace.

## Explicit profiles and prerequisites

The harness refuses missing or unsafe profiles before reserving artifacts or
starting Node/Due, including in dry-run mode. This is a narrow caller admission
check, **not** an effective native-policy or sandbox verifier.

Profiles must contain `schema_version: 1`, `provider`, explicit boolean
`ephemeral`, and `native_args`. Only optional nonempty `model` and
`effort: low|medium|high` may be added. No inherited prompt, schema, cwd, resume,
timeout, arbitrary flags, or extra profile fields are accepted. Native argument
lists are preserved in full; the harness adds only the requested cwd and schema
to a new private request snapshot. It does not edit the original profile.

For Codex, use the caller-reviewed
[workspace profile](../codex-workspace-profile.json), or a protected copy that
changes `sandbox_mode` to `"read-only"`. The exact eight `-c` entries required
by this smoke are:

```text
sandbox_mode="workspace-write"       (or "read-only")
approval_policy="never"
agents.enabled=false
features.multi_agent_v2=false
sandbox_workspace_write.network_access=false
sandbox_workspace_write.writable_roots=[]
sandbox_workspace_write.exclude_slash_tmp=true
sandbox_workspace_write.exclude_tmpdir_env_var=true
```

Order may vary; duplicates and any other entries are refused. Both native agent
controls must be explicit. The simple `examples/profile.json` and Node review
request do not meet these admission rules. The harness does not weaken them for
older CLIs. Verify that the exact installed Codex version supports and actually
honors them; inherited tools, connectors, and execution rules also require caller
review. A disabled built-in agent feature alone does not prevent all subprocess
delegation. Use an already authorized Git workspace; no Git trust bypass is
added or inferred.

For Claude, create a caller-owned profile with this exact restricted option set
(order may vary). The harness never writes it for you:

```json
{
  "schema_version": 1,
  "provider": "claude",
  "ephemeral": true,
  "native_args": [
    "--permission-mode", "dontAsk",
    "--permission-prompts", "none",
    "--tools", "",
    "--strict-mcp-config",
    "--mcp-config", "{\"mcpServers\":{}}"
  ]
}
```

This requests no built-in tools (including Agent) or MCP servers. Native option
support and effective restrictions need verification against the exact installed
Claude version. No bypass mode, allow grant, settings file, or alternative tool
list is accepted. If the native CLI rejects the profile, preserve the failure
and stop; do not silently change it to get a result.

Have the selected provider already authenticated under existing authorized
settings. Never copy credentials or authentication/config files into evidence.
Keep the harness, Node launcher, inputs, and artifact parent caller-controlled,
outside worker writable roots. The harness rejects an artifact directory within
the selected workspace; it cannot discover all additional inherited grants.
Do not point it at an untrusted substitute Due executable.

## Caller-owned invocation

First record the **exact** Due/native version output for this machine. The
harness requires it even for a dry run, but does not execute either version
command or claim to have verified the supplied strings. The executable path,
Due binary hash, provider and supplied versions are saved in `metadata.json`.
Use the chosen provider's version, and match the plan's executable to that
installation. Record requested model/effort in the profile when needed; missing
values remain unknown, and requested aliases do not prove observed settings.

For an already authorized Codex installation, this is a **plan-only** example:

```sh
due_bin=/path/to/installed/due
profile=/path/to/caller-owned/profile.json
workspace=/path/to/authorized-disposable-git-workspace
artifact_parent=/path/to/private-caller-artifacts
due_version=$("$due_bin" --version)
native_version=$(codex --version)
python3 -B examples/native-schema-smoke/smoke.py --dry-run \
  --due "$due_bin" --due-version "$due_version" --native-version "$native_version" \
  --profile "$profile" --workspace "$workspace" \
  --artifacts "$artifact_parent/minimal-plan-1" --case minimal
```

The artifact parent must exist. `--dry-run` runs only Due's planning path, with
no provider turn or network use. Inspect its plan and exit before the caller's
separately authorized live invocation. To run **one live turn**, explicitly use
`--live` and a **new** artifact path, such as `minimal-live-1`. It saves another
plan of that invocation before the one provider launch. A failed/malformed plan
or cancellation prevents that launch. To probe a negative case, explicitly select
`--case dialect` or `--case keyword` with another new path. No live runs were
performed as part of implementing or testing this example.

For Claude, use the Claude profile and exact `claude --version` output. Provider
versions previously mentioned elsewhere in this repository are historical
context, not a substitute for version evidence from the installation being used.

The harness sets no timeout: the caller must monitor `probe.run/trace.jsonl` and
the process, and can cancel. SIGINT, SIGTERM, and SIGHUP are forwarded once to
Node, which forwards to Due and waits for cleanup. Repeated signals are ignored;
there is no automatic hard kill, process-group kill, retry, or artifact deletion.
Cancellation is finalized before writing `summary.json`; later signals do not
change that snapshot. Due owns native provider cancellation. SIGKILL, host failure,
or an uncooperative child can leave incomplete artifacts. Do not remove an
artifact directory while a process may still use it. After all processes stop,
inspect partial effects and retain private evidence before caller-directed
cleanup; fresh attempts always need a fresh path and authorization.

## Read evidence as separate outcomes

Every artifact directory is reserved exclusively with mode 0700; files are 0600.
The Node caller also reserves `probe.plan/` and `probe.run/` exclusively. Due's
temporary schema/output files use the private `tmp/` outside the worker workspace.
No existing artifacts or original inputs are overwritten.

| Artifact | Meaning |
| --- | --- |
| `profile.original.json`, `schema.original.json`, `probe.prompt.txt` | Exact input snapshots; schema/profile/prompt hashes are in `metadata.json`. |
| `probe.request.json` | Resolved request containing the original native arguments plus absolute cwd/schema. |
| `probe.plan/plan.json`, `stderr.log`, `exit.json` | Due plan, diagnostics and raw exit/signal record from Node. |
| `probe.run/result.json`, `stderr.log`, `exit.json`, `trace.jsonl` | Live raw result, diagnostics, exit/signal and trace if tracing initialized. |
| `plan.launcher.*`, `run.launcher.*` | Node launcher's own stdout/stderr, separate from Due streams. |
| `answer-validation.json` | Independent answer validation, including invalid-input evidence for a malformed/empty result. Absent on plan-only runs. |
| `summary.json` | Node plan/run exits, independent validation exit, received signal, and overall harness exit. |

A nonzero Node/Due exit is preserved even if the answer validates. With successful
execution, invalid/missing answers yield exit 1 and validation input errors yield
exit 2. Preflight/harness errors yield exit 2; partial filesystem failures may
prevent a complete summary. Saved raw `exit.json` remains authoritative about
the child exit versus signal. Native rejection, authentication, permissions,
intake failure, cancellation, and malformed answers are different failure layers.
An answer-validation pass must never turn a failed execution into a success.

`native_compatibility` always stays `not_established_by_harness`. A successful
plan establishes only Due intake/transport planning. A live success with a valid
answer is evidence for that schema and exact environment, not every keyword or
proof that the provider enforced a constraint. A negative-case rejection needs
native diagnostics identifying the rejected dialect/keyword; an unrelated failure
does not establish incompatibility. The caller must inspect trace completeness,
permissions, actual effects, versions, and the independent validation before
recording its own bounded compatibility conclusion. No verdict or conclusion is
published by this example.

Plans, native argv, prompts, results and traces are **private**, even for synthetic
tasks. Share only sanitized findings, hashes, settings and aggregate outcomes
when separately authorized; never publish raw artifacts or private paths.

## Offline checks

```sh
python3 -B -m unittest discover -s examples/native-schema-smoke -p 'test_*.py' -v
node --check examples/native-schema-smoke/fake-due.fixture.mjs
```

The tests pass an executable fake Due to the unchanged Node launcher. They cover
exact schema/prompt/argument transport, plan-only operation, unsafe profile
rejection, private exclusive artifacts, separate execution/validation outcomes,
failure without retry, and cancellation through both launchers. The fake trace
is only a transport fixture, not native trace or compatibility evidence. All
temporary files live below this example and are cleaned after test completion.
Only fixture tests have short test deadlines; the real harness has none.

## Bounded live evidence, 2026-10-13

The outer caller executed these cases through the unchanged installed Due 0.1.0
on macOS 27 arm64, PATH Codex CLI 0.162.0 and Claude Code 2.1.296. The original
schema bytes and the no-tools prompt were identical across provider arms. Original
profile snapshots and saved request native arguments were independently matched
and hashed. Codex used all eight checked-in policy entries; Claude used
the exact tool-less option set above. Profiles
used the explicit policies above; model/effort were omitted and observed identity
remains unknown. Provider interfaces/defaults differ, so this is compatibility
evidence rather than a model-quality or equal-compute comparison.

| Case | Claude Code 2.1.296 | Codex CLI 0.162.0 |
| --- | --- | --- |
| Minimal | Successful execution; independently valid answer | Successful execution; independently valid answer |
| Explicit 2020-12 declaration | Native rejection: dialect key/ref unavailable; exit 1 | Successful execution; independently valid answer |
| `not` keyword | Successful execution; independently valid answer | Native strict-schema rejection naming `summary.not`; exit 1 |

All six plans succeeded; four real calls succeeded and two preserved native
rejections. Original schemas were unchanged, no retries or translation occurred,
and independent envelope/answer/trace verification was performed by the caller.
Complete delivered traces do not establish a permission audit. A valid positive
answer does not prove native enforcement of `not`; it establishes acceptance and
that the observed answer satisfies the supplied schema. Raw artifacts remain
private. These results do not establish acceptance for other schemas, providers,
versions, models or machines. The harness itself deliberately emits no compatibility
verdict. A later stricter plan guard was independently checked against all six retained
real plans. The provider-launch path was unchanged; later malformed-plan handling
was checked with an offline fake fixture. These source revisions and live snapshot
evidence are recorded separately rather than presented as one identical build.
