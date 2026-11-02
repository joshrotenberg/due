# A supervised assignment in one resident session

`supervised.mjs` composes the existing session client into an implement, verify,
revise workflow. The caller supplies the assignment, independent verifier, and
optional checkpoint decisions. The same native session handles all corrective
turns. No workflow engine or additional RPC method is needed.

```js
import { join } from 'node:path';
import { withSession } from './session.mjs';
import { supervisedWorkflow } from './supervised.mjs';
import { verifyParser } from './project-checks.mjs'; // Your independent check.

const artifactDir = '/path/to/fresh-artifacts';
const outcome = await withSession({
  backend: 'rpc',
  dueBinary: '/path/to/due',
  profile: {
    request: '/path/to/codex-workspace-profile.json',
    cwd: '/path/to/disposable-checkout',
  },
  artifactDir,
  maxTurns: 3, // Initial implementation plus at most two corrections.
  onItem: record => console.log(JSON.stringify(record)),
}, agent => supervisedWorkflow(agent, {
  id: 'parser-repair',
  task: 'Fix the parser regression described in issue 123 and add a focused test.',
  verificationFile: join(artifactDir, 'verification.jsonl'),
  maxRevisions: 2,
  async verify({ signal }) {
    // Inspect the workspace and run your own assertions. A worker's report that
    // tests passed is not verification. Join any check processes before returning.
    const check = await verifyParser({ signal });
    return { accepted: check.passed, feedback: check.summary };
  },
  async checkpoint({ attempt, verification, signal }) {
    signal.throwIfAborted();
    // Replace this policy with a parent-agent or human decision if needed.
    return attempt === 1 || verification.accepted === false;
  },
}));

if (!outcome.accepted) throw new Error(`Assignment was not accepted: ${outcome.status}`);
```

`verifyParser` is caller-owned project code. Its result must come from actual
checks, not parsing the worker's final answer for a success claim. The included
offline test demonstrates the distinction: a fake worker writes an incorrect
file while claiming success; a separate verifier reads that file, rejects it,
then accepts the corrected contents after a second turn.

`verify` receives `{attempt, turnId, result, signal}` and returns
`{accepted: boolean, feedback: string}`. `result` is a copy of the native turn
result for reference. Feedback is required, limited to 64 KiB, and included as
quoted check output in the next corrective prompt. A native status other than
`succeeded` stops before verification. A thrown verifier or transport error ends
the workflow without another turn.

The optional `checkpoint` runs before each proposed worker turn and must return
`true` to continue or `false` to stop. It receives the prompt and previous
verification, allowing a parent to approve the next step. A checkpoint stop is
an unaccepted outcome, including when it occurs before the first turn.

The result separates execution from acceptance:

| `status` | Meaning |
| --- | --- |
| `accepted` | The independent verifier accepted a successful worker turn. |
| `verification_failed` | Verification still failed when the revision budget ended. |
| `worker_failed` | A worker turn failed, timed out, or was cancelled. |
| `stopped` | The caller declined a proposed turn at a checkpoint. |

Only `accepted` sets `outcome.accepted` to true. Native turn success does not
imply assignment acceptance. Ordinary rejected verification is saved as data;
callback/protocol failures throw. `maxRevisions` is 0 through 63 and must fit the
surrounding session's remaining `maxTurns` budget. Give each workflow a distinct
`id` when working several related assignments in one session.

The exclusive, private `verificationFile` records worker statuses, independent
verdicts, checkpoints, and terminal workflow outcome. It is separate from the
session's raw stream, turn results, and process receipts. Existing evidence is
never overwritten. The receipt is bounded to 8 MiB. Task and feedback text may
contain sensitive source information.

Callbacks receive the scope's cancellation signal, combined with an optional
workflow `signal`. They must observe cancellation, bound their own operations,
and join any processes/tasks they start. The workflow awaits callback settlement;
it cannot preempt arbitrary JavaScript or settle a caller's detached processes.
A verification result arriving after cancellation cannot accept the assignment
or start another turn. Gracefully interrupting a worker turn is distinct from
cancelling the whole session.

This example runs in Node as an independently implemented caller. The supported
[embedded JavaScript harness](../../docs/harness.md) now ships as `due script`,
with a [matching assignment example](../harness/supervision.js). Its Rust owner
provides command deadlines, output bounds, cancellation and settlement without
a Node runtime. The raw RPC interface remains available to custom clients; the
[Lua comparison](../rpc-languages/README.md) is historical, not a product runtime.

Run the deterministic examples without providers or credentials:

```sh
node --test examples/session-harness/supervised.test.mjs
```
