---
name: profiles
description: Write and reuse versioned JSON profiles across due run, due script and raw RPC, including precedence, path rules and which native options each surface accepts.
---

# Write and reuse JSON profiles

A profile is a version-1 JSON file you own. Due has no profile search path, no
inheritance between files, no registry and no config store. You pass the file
explicitly, and every surface reads the same format:

- `due run --request FILE`
- `scope.open({request: "FILE"})` in a `due script` workflow
- `session.open` with `"request": "FILE"` in a raw `due rpc` client

Omit permission options to use the provider's ordinary user, project and
managed settings for the worker. Profiles do not copy the calling session's
effective permission mode or temporary grants. Add native controls only when
the assignment calls for them.

Keep profiles in a caller-owned location. If a profile carries permission
settings, store it outside every directory the worker can write, so the worker
cannot edit its own limits.

## Fields

Only `"schema_version": 1` is required in the file. Unknown or duplicate keys,
wrong JSON types, malformed JSON and unsupported versions are rejected, even if a
flag or parameter would have replaced the field.

| Field | Value |
| --- | --- |
| `provider` | `claude` or `codex` (never null) |
| `prompt` | Literal string (never null). Rejected by RPC and script: prompts belong to turns |
| `model` | Nonempty native model string, or null for the provider default |
| `effort` | `low`, `medium`, `high`, or null |
| `cwd` | Directory string or null |
| `timeout` | Positive duration such as `30s` or `5m`, or null for no deadline |
| `schema` | Answer-schema file path, or null. Used by `due run` only; RPC rejects a non-null value |
| `resume` | Nonempty native session ID, or null |
| `ephemeral` | Boolean, default false. True conflicts with a non-null `resume` |
| `native_args` | Ordered array of strings, default empty |

`due schema request` prints the installed structural schema. A partial profile is
normal:

```json
{"schema_version":1,"provider":"codex","effort":"medium","ephemeral":false}
```

## Precedence

Highest to lowest, per field: an explicit flag or parameter, then the profile
file, then the default. Omission inherits; nothing is merged key by key inside
`native_args`.

| Surface | Override source | Clearing a profile value |
| --- | --- | --- |
| `due run` | CLI flags (`--provider`, `--model`, `--effort`, `--cwd`, `--timeout`, `--schema`, `--resume`, `--prompt`/`--stdin`) | Edit the file or set the field null; `--no-ephemeral` overrides `ephemeral: true`; `--clear-native-args` empties the list |
| `due rpc` | Named parameters of `session.open` | Pass `null` for `model`, `effort`, `cwd`, `timeout`, `resume`; `"ephemeral": false`; `"native_args": []` |
| `due script` | The object passed to `scope.open`, forwarded to `session.open` | Same as RPC |

`due run`'s timeout is an absolute limit from provider start. A profile
`timeout` applies whenever `--timeout` is omitted. In RPC and script sessions
it is a per-turn deadline, and a script's own `--timeout` is separate.

In a resident session, expiry also fails the native session: the `timed_out`
turn result is followed by `session_failed`, and another turn on that handle is
rejected. In `due script`, this fails the workflow. Close the failed handle before
opening another session. Resume requires saved, non-ephemeral native history;
otherwise start fresh. A session's successful cleanup does not restore usability.

Native arguments replace as a whole list everywhere. Supplying `-- ARGS` to
`due run`, or `native_args` to `session.open`, discards the profile's list,
including any permission grants and restrictions. Copy the complete intended
list when you change one entry.

Switching `provider` keeps the inherited `native_args` and validates them for the
new provider; Due never silently drops an option. Review inherited policy when
you change provider.

## Paths

- `due run`: `cwd` and `schema` in the file resolve from the caller's current
  directory, not from the profile's directory. `--request -` reads the whole
  document from stdin and cannot be combined with `--stdin`.
- `due rpc`: the `request` path and `cwd` resolve from the directory Due was
  started in. `request: "-"` is rejected because stdin carries RPC. The file must
  be a regular UTF-8 file of at most 1 MiB, read once at open.
- `due script`: `--cwd` sets the RPC process directory, so `request` paths
  resolve from `--cwd`, not from the script or the profile.

Prefer absolute paths in profiles you share between callers.

## Native options per surface

`native_args` carries provider options through unchanged. The surfaces accept
different sets.

`due run` forwards any provider option after the shared ones. Due reserves prompt
transport, output, cwd, session selection, persistence, help and version, and
rejects a native model, effort or schema option when the shared equivalent is set.
Passthrough takes options only, not a command or positional prompt. It is not a
security boundary.

Resident sessions (`due rpc`, `due script`) accept only an audited set, and
reject anything else before the provider starts:

- Codex: `-c` / `--config` with one `key=value` TOML assignment per use. Allowed
  keys: `sandbox_mode`, `approval_policy`, `agents.enabled`,
  `features.multi_agent_v2`, `features.multi_agent`, and
  `sandbox_workspace_write.network_access`, `.writable_roots`,
  `.exclude_slash_tmp`, `.exclude_tmpdir_env_var`.
- Claude: `--allowedTools` / `--allowed-tools`, `--disallowedTools` /
  `--disallowed-tools`, `--permission-mode`. One value each; join several tool
  patterns with commas. Modes: `default`, `acceptEdits`, `plan`, `dontAsk`,
  `bypassPermissions`.

So a profile written for `due run` that uses `--max-turns`, `--tools`,
`--permission-prompts` or other Codex config keys works for `run` but fails to
open a resident session. Keep a separate profile for each surface when needed.
Native permissions stay in force and are not made equivalent between providers.
Do not add bypass modes as a default.

## Examples

Codex workspace profile with no extra writable roots, no network and no default
temporary grants. It uses only keys that run, script and RPC all accept:

```json
{
  "schema_version": 1,
  "provider": "codex",
  "ephemeral": false,
  "native_args": [
    "-c", "sandbox_mode=\"workspace-write\"",
    "-c", "approval_policy=\"never\"",
    "-c", "agents.enabled=false",
    "-c", "features.multi_agent_v2=false",
    "-c", "sandbox_workspace_write.network_access=false",
    "-c", "sandbox_workspace_write.writable_roots=[]",
    "-c", "sandbox_workspace_write.exclude_slash_tmp=true",
    "-c", "sandbox_workspace_write.exclude_tmpdir_env_var=true"
  ]
}
```

Keep caller-owned profiles, answer schemas and evidence outside worker-writable
roots. This profile excludes the inherited TMPDIR write grant; commands needing
temporary files may need an explicitly authorized workspace-local TMPDIR on
those commands. Do not export a worker-writable TMPDIR to Due when protecting
its own temporary schema/output files.

It grants workspace edits and sandboxed commands, not a single file. Provider API
access, inherited native rules and managed requirements remain separate.

Claude profile that permits reading and one test command, and
denies anything else that would need approval. Resident sessions require one
comma-joined value per option, as here; check the provider's help for how your
installed Claude CLI parses it:

```json
{
  "schema_version": 1,
  "provider": "claude",
  "native_args": [
    "--permission-mode", "dontAsk",
    "--allowedTools", "Read,Bash(python3 -B -m unittest -v)"
  ]
}
```

The allowlist must match the command as it runs after any hook rewrites it.
For example, a hook that adds a flag could turn `git status` into
`git --no-pager status`. Probe with a one-line `due run` using the profile:
ask the worker to run one allowed command and check that the command ran.
`permission_denials` appears only in `due run` results; when present it names
the denied tool, and a detailed trace can show the Bash input checked if
Claude reported it. Its absence does not prove the command ran, and the trace
does not audit what a hook executed.

Use the profile from each surface:

```sh
# One-shot: flags override the file, native args after -- replace the whole list.
due run --request /caller-owned/claude.json --cwd "$PWD" --stdin --dry-run --output json < task.txt
due run --request /caller-owned/claude.json --cwd "$PWD" --stdin --output json < task.txt
due run --request /caller-owned/claude.json --stdin --effort high --timeout 30m < task.txt
```

```js
// Save as review.js; pass {"task":"..."} with --args or --args-file.
Due.run(async scope => {
  const worker = await scope.open({
    request: '/caller-owned/claude.json',
    cwd: '/path/to/authorized/project',
    effort: 'high',
  });
  const turn = await worker.turn(Due.args.task);
  if (turn.status !== 'succeeded') throw Error('Worker execution failed');
  return {text: turn.text};
});
```

```json
{"jsonrpc":"2.0","id":1,"method":"session.open","params":{"request":"/caller-owned/claude.json","cwd":"/path/to/authorized/project","effort":"high","stream":true}}
```

For RPC and script, the success result and its `session_opened` item carry
`resolved_options`: the merged options, canonical cwd and the complete native
argument list. Keep it in your receipt. It records what was requested, not the
provider's effective permissions. Use `due run --dry-run` to inspect a merge
before a one-shot run; there is no dry run for `session.open`.

## Checklist

1. Start from `{"schema_version":1,"provider":"..."}` and add only what you need.
2. If the assignment needs explicit permission settings, use `native_args`.
3. Preview a `due run` merge with `--dry-run --output json`.
4. Check required restrictions with a harmless probe on the actual surface,
   including its cwd and hooks. Resident sessions reject some options that
   `due run` accepts. See the native permissions section of `run`.
5. See `due skills show run`, `supervise` and `rpc` for each surface in depth.
