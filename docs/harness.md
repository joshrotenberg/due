# JavaScript supervision

The installed reference is available offline with `due skills show supervise`.
Use `due -h` for available commands and `due COMMAND --help` for detailed examples.

`due script` runs a trusted JavaScript workflow inside Due. It keeps one Claude
Code or Codex session alive while the caller observes items, makes explicit
permission decisions, checks artifacts independently and decides whether to
request another turn. The native CLI still runs its own tools within each turn.
Rust owns the processes, bounded I/O, cancellation and cleanup.

Normal builds include the harness and raw RPC. No Node, npm or browser runtime is
required. Use `due run` for one turn and `due run --resume ID` for separate
invocations that only need saved native context. Use `due rpc` from any language
when writing your own client. The [wire contract](rpc.md) is independently usable;
JavaScript is a convenience layer above it.

## Run an assignment

The [complete example](../examples/harness/supervision.js) implements a bounded
implement, verify, revise loop. Its verifier command and acceptance criteria are
caller-owned; worker output never chooses a program to execute.

```sh
due script examples/harness/supervision.js \
  --cwd /path/to/authorized/project \
  --evidence /tmp/due-assignment-001 \
  --args-file /caller-owned/assignment.json > result.json
```

For example, the argument file contains:

```json
{
  "profile": {"request": "/caller-owned/worker.json", "cwd": "/path/to/authorized/project"},
  "task": "Fix the parser regression and add a regression test.",
  "verify": {"program": "cargo", "args": ["test", "--locked"], "timeout_ms": 120000},
  "max_revisions": 2
}
```

The referenced worker file is an existing Due version-1 JSON profile. It selects
the provider and native execution policy. RPC uses the same
[profile resolution rules](rpc.md#reusing-json-profiles), with explicit open
fields overriding file values. Only supported resident options are accepted;
unsupported one-shot options fail rather than disappearing silently.
The example also accepts `turn_timeout_ms` for each worker wait and
`permission_check` for an optional caller-owned check before denying an interaction.

`--args JSON` and `--args-file FILE|-` are mutually exclusive; the default is `{}`.
The latter reads JSON from a file or stdin. Script, argument-file and evidence
paths resolve from the invocation directory. Input and resolved working-directory
paths must be valid UTF-8; unsupported paths fail before execution. `--cwd` resolves from there too and
becomes the RPC process and default verifier directory. Profile request paths
resolve from the RPC directory, not the profile's directory. An explicit native
session `cwd` does not change the verifier's default directory.

The evidence directory must be new and its parent must already exist. `--timeout` defaults to `5m` and covers script
evaluation and waits; input preparation happens first. Choose a deliberate budget
for the assignment. Cleanup has additional bounded settlement time. The whole-workflow deadline,
`worker.wait`/`worker.turn` wait timeout and native profile turn timeout are
separate. Raising `--timeout` does not extend the five-minute default worker wait;
pass a deliberate second `timeoutMs` argument for a longer turn.

## The supported script API

Write one classic JavaScript file using `Due.run`:

```js
Due.run(async scope => {
  const worker = await scope.open(Due.args.profile, {
    onItem(item) { Due.log(JSON.stringify(item)); },
  });
  const turn = await worker.turn(Due.args.task);
  if (turn.status !== 'succeeded') throw Error('Worker execution failed');
  const check = await scope.exec(Due.args.verify, scope.signal);
  if (check.error != null || check.status !== 'succeeded') {
    throw Error('Independent verification failed');
  }
  return {accepted: true, run_id: turn.run_id, check};
});
```

This small version fails on an unexpected permission interaction. The complete
example includes provider-specific denial and bounded revision handling.

| API | Behavior |
| --- | --- |
| `Due.args` | Parsed caller JSON input |
| `Due.log(text)` | Diagnostic string on stderr |
| `Due.run(async scope => ...)` | One root workflow; its JSON return value is the script result |
| `scope.open(profile, callbacks?)` | Open the scope's single resident worker; returns a worker with `opened` details |
| `scope.exec(spec, signal?)` | Run and settle a caller-selected command; default signal is the scope's |
| `scope.sleep(milliseconds, signal?)` | Yield while session items and managed work progress |
| `scope.signal`, `scope.cancel(reason)` | Observe or request sticky scope cancellation |
| `worker.start(prompt)` | Admit one turn and return its Due turn ID |
| `worker.wait(turnId, timeoutMs?)` | Await and consume that turn's terminal result; default wait is five minutes |
| `worker.turn(prompt, timeoutMs?)` | Start then wait for one turn |
| `worker.interrupt(turnId)` | Request interruption of the exact active turn and await acknowledgement; resolves `{interrupt_requested:false, turn_id, already_finished:true}` when that turn already finished and `wait` has not consumed it, including when its terminal item arrives before Due's finished-turn rejection of the outstanding request; other rejections still throw |
| `worker.reply(item, response)` | Send a native response for an unresolved interaction and await acknowledgement |
| `worker.close()` | Close explicitly; scope exit also closes automatically |

Await managed operations. After interruption, still await the terminal turn
before starting the next one. A Due turn ID is not a native session/thread ID.
An ordinary failed native turn is result data, not a thrown JavaScript error.
The caller must inspect it before treating work as accepted.

An expired `worker.wait` or `worker.turn` timeout fails the whole scope. Catching
it does not restore the scope, so a later interrupt or verifier also rejects;
the script result is null. Choose the wait budget deliberately. Interrupt and
then wait to stop a turn before that budget expires.

The profile's per-turn timeout is a separate deadline. Its expiry produces a
`timed_out` turn followed by `session_failed`, makes the handle unusable and
fails the script. A later invocation can start fresh or explicitly resume native
history only if it persisted and was not ephemeral.

There are no module imports, Node/npm APIs, browser APIs or file helpers. Use
caller-selected `scope.exec` commands for checks that need filesystem access.
The comparison runner's `bridge` object and scheduler internals are not public
harness APIs. The [language comparison](rpc-languages.md) records the decision;
Lua remains a historical comparison, not a supported `due script` language.

## Items, permissions and cancellation

`onItem(item)` receives streamed session items, including native provider
payloads exposed by Due. It must finish synchronously; returning a Promise fails.
The Rust reader continues independently and the JavaScript scheduler pumps items
while awaited commands, sleeps and permission callbacks remain pending.

`onInteraction(item, signal)` may be async and must return an explicit native
response object. Choose its shape using the resolved provider in
`worker.opened.provider`; do not infer provider identity from an optional input
field that may have been inherited from a profile. For example, caller-authorized
denial uses `{behavior: 'deny', message: 'Not authorized'}` for Claude, or
`{decision: 'decline'}` for a Codex `requestApproval` interaction. Other interaction
kinds need their own supported response or a failure.

Pass the supplied interaction signal to its awaited work:

```js
async function onInteraction(item, signal) {
  await scope.exec(callerPermissionCheck, signal);
  signal.throwIfAborted();
  return callerDecision(item);
}
```

Here `scope`, the checker and decision policy belong to the surrounding caller
workflow. While a decision or its reply is pending, retirement, turn completion
or session closure aborts that interaction's signal, cancels its managed checker
and prevents a late decision. Once the callback and reply settle, their signal
is no longer tracked. A reply already sent retains its acknowledgement ownership.
Only an exact known stale-reply
error after matching retirement is ignored; a distinct callback or reply failure
still fails the workflow.

`scope.cancel(reason)` remains failure even when caught. Native session failure
also cancels verification between turns. Returning with unfinished turns,
unjoined managed work, unresolved Promises or unhandled rejections fails. Scope
cleanup closes the worker and joins managed callbacks and commands; Rust settles
owned processes even if guest cleanup fails. Cancellation is not rollback.

## Verifier commands and limits

The command specification accepts only `program`, string-array `args`, optional
`cwd`, `timeout_ms` and `max_output_bytes`. Commands inherit the runner environment,
have closed stdin and use no implicit shell. An explicit command `cwd` resolves
against the runner's `--cwd`.

Results contain `status`, `exit_code`, `signal`, `stdout`, `stderr` and `error`.
Normal nonzero exits can inform a caller-authorized revision. Timeouts,
cancellation, invalid output and infrastructure failures must not be treated as
ordinary failed tests. Cancellation through a signal rejects the await after
process settlement. The example limits revisions and never automatically retries
an infrastructure failure.

| Limit | Value |
| --- | --- |
| Command deadline | 30 seconds by default; at most five minutes |
| Combined stdout/stderr | 64 KiB by default; at most 1 MiB |
| Retained command results | At most eight |
| JavaScript heap | 32 MiB |
| Script source and JSON return value | At most 1 MiB each |
| Raw Due stdout evidence | 64 MiB retained prefix; overflow fails |
| Command receipt | 64 MiB; overflow or write failure fails |
| Session stream | An omitted item, dropped push or cursor gap fails the scope; no automatic recovery |

Session items larger than 4 MiB become `omitted`. Stream failure withholds the
script result. A raw RPC client can implement journal/result reconciliation via
`session.read` and `turn.read`, but an oversized native item replaced by an
omission cannot be recovered from the journal. The latest terminal result and
pending interactions are retained separately for `turn.read`.

Invalid UTF-8 command output fails explicitly; invalid stream text is unavailable
instead of silently replaced. Unix commands receive their own process groups.
Cleanup signals the group before reaping its leader and drains both pipes within
deadlines, with a shared seven-second command shutdown budget. The Due RPC child
then has up to 90 seconds to finish its own native startup/control requests and
provider cleanup. It exits as soon as that work settles; this is not a mandatory
wait. Forced-kill and reader settlement each have a further two-second budget,
so cleanup can add roughly 101 seconds beyond the workflow deadline. OS reaping
is not a hard real-time guarantee. A forced Due kill or failed settlement is
reported as failure, with native settlement unconfirmed. `receipt.settled` means
the Due child was reaped, not that every native descendant was proven gone.
Escaped process groups are not supervised; non-Unix builds only control direct
children.

These are trusted scripts. Provider sandbox and permission settings do not
constrain host verifier commands. Never turn a worker's proposed command into
an execution request without the caller's authorization. JavaScript deadlines
are cooperative at runtime checkpoints; this is not an untrusted-code isolation
boundary or a guarantee about arbitrary detached descendants.

## Results and evidence

Stdout always contains one JSON document after CLI argument parsing. Logs and
diagnostics use stderr. Export the exact schema offline with
`due schema script-result`; the envelope contains:

- `schema_version: 1` and the installed `cli_version`.
- `status`: `succeeded` or `failed`.
- `result`: the root workflow's strict JSON return value on success, otherwise null.
- `error`: null on success, or a failure string.
- `duration_ms`: measured command duration.
- `evidence`: absolute paths `directory`, `stdout`, `process`, `commands` and `workflow`;
  null when the invocation/evidence path cannot be resolved or represented as UTF-8.

An omitted return becomes null. Functions, cycles, non-finite numbers, unsafe
integers and oversized results fail. No successful result is published until
cleanup succeeds. A script that ignores failed turns or checks can still return
normally; execution success does not invent an acceptance verdict.

Exit 0 means script success, exit 2 means invalid input, and exit 1 covers setup,
workflow, cancellation and cleanup failures. CLI syntax errors print stderr and
exit 2 before creating the JSON envelope. These codes differ from `due run`.

The new evidence directory contains:

| File | Evidence |
| --- | --- |
| `stdout.bin` | Original Due stdout bytes, captured before JavaScript decoding |
| `receipt.json` | Owned Due process exit, forced-kill and transport settlement |
| `exec.jsonl` | Command specifications, spawned PIDs and bounded settled results |
| `workflow.json` | Workflow outcome, result and cleanup outcome |

The process receipt separates the original `transport_error` from a
`cleanup_error`. `rpc_shutdown_confirmed` is true only after a successful natural
Due exit without forced termination; false means native shutdown is unconfirmed
even if `settled` shows the Due child was reaped. These fields do not prove that
arbitrary descendants outside owned process groups are gone.

On early failure, report paths may name files that were never created. Treat each
receipt as evidence of its stated scope; Due stdout is not a promise to expose
every native provider byte. Parsed JavaScript numbers have normal safe-integer
limits, while raw transport bytes retain their original representation. Item
gaps or lost observation fail this client; it does not silently repair them with
cursor replay or reconnect after process loss.

These files can contain source, paths, prompts and tool output. Keep them private;
share sanitized reproductions under the [reporting guide](dogfooding.md). Check
actual diffs, artifacts and authorized external effects before reporting completion.
