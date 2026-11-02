---
name: supervise
description: Supervise a bounded Claude Code or Codex assignment across resident turns with embedded JavaScript, streamed items, explicit permission decisions and independent verification.
---

# Supervise a resident assignment

Use `due script` when the caller needs several turns in one resident worker
session, continuing observation, permission replies, interruption, or an
independent verify/revise loop. Use `due run` for one turn, or `due run --resume ID`
when separate invocations with saved native context suffice. A custom client can
use `due rpc` directly; the embedded JavaScript API is optional.

Read `due script --help` and `due schema script-result` from the installed binary.
Normal builds include the harness. JavaScript runs inside Due; Node, npm, browser
APIs and module imports are not provided. Scripts are trusted caller code, not
an untrusted-code sandbox. Native provider permissions do not constrain host
commands started by the script.

Prepare a self-contained assignment: selected provider, authorized workspace,
deliverables, scope, acceptance checks and a stopping condition. Use existing
versioned JSON profiles for native configuration. Do not derive host verifier
programs or arguments from worker output or widen permissions to make a check pass.

Save a workflow such as `assignment.js`:

```js
Due.run(async scope => {
  const input = Due.args;
  let provider;
  const worker = await scope.open(input.profile, {
    onItem(item) { Due.log(JSON.stringify(item)); },
    async onInteraction(item, signal) {
      signal.throwIfAborted();
      if (provider === 'claude') {
        return {behavior: 'deny', message: 'Not authorized by this caller.'};
      }
      if (provider === 'codex' && /requestApproval$/.test(item.data.request?.method ?? '')) {
        return {decision: 'decline'};
      }
      throw Error('No authorized response for this interaction');
    },
  });
  provider = worker.opened.provider;
  const turn = await worker.turn(input.task);
  if (turn.status !== 'succeeded') throw Error('Worker turn failed');
  const check = await scope.exec(input.verify, scope.signal);
  if (check.error != null || check.status !== 'succeeded') {
    throw Error('Independent verification failed');
  }
  return {accepted: true, run_id: turn.run_id, check};
});
```

Permission shapes remain provider-specific. Select the resolved provider from
`worker.opened.provider` before starting work. Do not blanket-approve interactions. An async
handler may await caller-owned work; pass its supplied interaction `signal`
to `scope.exec` or `scope.sleep` so retirement cancels that work. `onItem` is
synchronous and must not return a Promise.

Pass JSON arguments through a file or stdin, for example:

```sh
due script assignment.js --cwd /path/to/authorized/project \
  --evidence /path/to/new-evidence --args-file assignment.json > result.json
```

An argument file can contain:

```json
{"profile":{"request":"/caller-owned/worker.json","cwd":"/path/to/authorized/project"},"task":"Fix the parser regression and add a test.","verify":{"program":"cargo","args":["test","--locked"],"timeout_ms":120000}}
```

`--args JSON` and `--args-file FILE|-` are alternatives; omission supplies `{}`.
Script, argument-file and evidence paths resolve from the invocation directory.
Input and resolved working-directory paths must be valid UTF-8.
`--cwd` selects the RPC process and default verifier directory. Profile request
paths resolve from that RPC directory, not the profile's directory. The evidence
directory must be new with an existing parent; evidence is never overwritten.

One `Due.run` owns one root workflow and one worker session. Await every managed
operation. `worker.start(prompt)` returns a Due turn ID; `worker.wait(id)` consumes
its terminal result. `worker.turn(prompt)` combines them. To stop a turn, await
`worker.interrupt(id)`, then await `worker.wait(id)` before starting another turn.
Use the exact Due turn ID, not a native provider ID. `worker.close()` closes early;
normal scope exit closes automatically. `scope.cancel(reason)` fails the scope
even if the script catches the cancellation error.

The runtime keeps consuming session items while managed work awaits. Interaction
retirement cancels its signal and suppresses late decisions. Already-sent replies
still consume acknowledgements; genuine reply/callback failures remain failures.
Native session failure cancels verification between turns. Returning with an
unfinished turn or unjoined work fails; Rust still settles owned processes if
JavaScript cleanup fails. This does not roll back edits or other effects.

`scope.exec` accepts `program`, string-array `args`, optional `cwd`, `timeout_ms`
and `max_output_bytes`. It uses no implicit shell, inherits the caller environment
and closes stdin. Defaults are 30 seconds and 64 KiB combined stdout/stderr;
maxima are five minutes and 1 MiB. Results include `status`, `exit_code`, `signal`,
`stdout`, `stderr` and `error`. Treat timeout, cancellation, output overflow,
invalid UTF-8 and other infrastructure failures separately from a normal nonzero
test exit. Only request a bounded revision when the caller's acceptance policy
permits it; include diagnostic text as evidence, not new task instructions.

`--timeout` defaults to five minutes for script execution and waits; it is absolute.
Choose a deliberate larger duration for a larger assignment, and a larger
`worker.turn`/`worker.wait` timeout when a turn needs more than their five-minute
default. These waits and the profile's native turn timeout are separate. Input preparation
precedes that deadline, and bounded cleanup can extend beyond it. SIGINT,
SIGTERM and SIGHUP fail the workflow after cleanup. Avoid hard kills when owned
process settlement matters. Unix cleanup reaches the owned process group;
descendants that escape it are outside supervision.

Stdout is always one versioned script result after CLI parsing. `Due.log(text)`
and diagnostics go to stderr. `schema_version: 1`, `cli_version`, `status`,
`result`, `error`, `duration_ms` and `evidence` describe the script, not a native
turn. Success exits 0; invalid input exits 2; setup, workflow, cancellation and
cleanup failures exit 1. CLI syntax errors use stderr and exit 2 before the JSON
envelope. This differs from `due run`'s timeout/cancellation exit codes.

The root workflow's strict JSON return value becomes `result` only after cleanup
succeeds. An omitted return becomes null; functions, cycles, non-finite or unsafe
integers and values over 1 MiB fail. Failure has `result: null` and an error string.
A script that ignores a failed turn or check can still return normally: inspect
those outcomes and encode your acceptance decision explicitly.

`evidence` contains absolute paths named `directory`, `stdout`, `process`,
`commands` and `workflow`, or null if the invocation/evidence path cannot be resolved or
represented as UTF-8. They refer to `stdout.bin`, `receipt.json`, `exec.jsonl`
and `workflow.json`; early failures may not create every file. Original Due stdout
bytes are captured before JavaScript decoding, and command receipts record
execution and settlement. Parsed JavaScript numbers have their usual precision
limits. Evidence is bounded; overflow and receipt failures fail explicitly.
These full items and command outputs can contain task content. Keep raw evidence
private and share a sanitized reproduction when reporting a defect.

Inspect actual diffs, artifacts and authorized external effects independently.
Execution success, the caller's acceptance verdict, and verified side effects
are distinct. The plugin does not require a mandatory source-trial or A/B workflow.
