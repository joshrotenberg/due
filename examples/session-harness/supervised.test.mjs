import assert from 'node:assert/strict';
import { chmod, copyFile, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { withSession } from './session.mjs';
import { supervisedWorkflow } from './supervised.mjs';

async function setup(t, model = 'repair', provider = 'codex') {
  const cwd = await mkdtemp(join(tmpdir(), 'due-supervised-'));
  t.after(() => rm(cwd, { recursive: true, force: true }));
  const dueBinary = join(cwd, 'due');
  await copyFile(fileURLToPath(new URL('./supervised.fixture.mjs', import.meta.url)), dueBinary);
  await chmod(dueBinary, 0o700);
  const artifactDir = join(cwd, 'artifacts');
  const options = { backend: 'rpc', dueBinary, artifactDir, maxTurns: 4, profile: { provider, cwd, model } };
  const workflow = { id: 'parser', task: 'Write 42 followed by a newline to answer.txt.',
    verificationFile: join(artifactDir, 'verification.jsonl'), maxRevisions: 2,
    async verify({ signal }) {
      signal.throwIfAborted();
      const actual = await readFile(join(cwd, 'answer.txt'), 'utf8');
      return { accepted: actual === '42\n', feedback: `answer.txt expected 42 followed by newline; got ${JSON.stringify(actual)}` };
    } };
  const lines = async path => (await readFile(path, 'utf8')).trim().split('\n').map(JSON.parse);
  return { cwd, options, workflow,
    records: () => lines(workflow.verificationFile),
    calls: () => lines(join(cwd, 'calls.jsonl')),
    receipt: async () => JSON.parse(await readFile(join(artifactDir, 'session.json'), 'utf8')),
  };
}

for (const provider of ['claude', 'codex']) {
  test(`${provider}: independent failed check produces a bounded revision in the same session`, async t => {
    const { cwd, options, workflow, records, calls, receipt } = await setup(t, 'repair', provider);
    const observed = [];
    options.onItem = item => observed.push(item);
    const outcome = await withSession(options, agent => supervisedWorkflow(agent, workflow));
    assert.equal(outcome.status, 'accepted');
    assert.equal(outcome.accepted, true);
    assert.equal(outcome.attempts, 2);
    assert.equal(outcome.result.text, 'Implemented. All tests pass.');
    const verifications = (await records()).filter(item => item.kind === 'verification');
    assert.deepEqual(verifications.map(item => item.accepted), [false, true]);
    assert.match(verifications[0].feedback, /41/);
    const turns = (await calls()).filter(call => call.method === 'turn.start');
    assert.equal(turns.length, 2);
    assert.equal(turns[0].params.session_id, turns[1].params.session_id);
    assert.match(turns[1].params.prompt, /Independent caller verification/);
    assert.match(turns[1].params.prompt, /41/);
    assert.equal((await readFile(join(cwd, 'launches.jsonl'), 'utf8')).trim().split('\n').length, 1);
    assert.ok(observed.some(item => item.data.kind === 'native'));
    assert.equal((await receipt()).native_session_id, 'supervised-native');
    assert.equal(await readFile(join(cwd, 'exited'), 'utf8'), 'yes');
    assert.equal((await stat(workflow.verificationFile)).mode & 0o777, 0o600);
  });
}

test('first successful verification accepts without a correction', async t => {
  const { options, workflow, records } = await setup(t, 'correct');
  const outcome = await withSession(options, agent => supervisedWorkflow(agent, { ...workflow, maxRevisions: 0 }));
  assert.equal(outcome.accepted, true);
  assert.equal(outcome.attempts, 1);
  assert.equal((await records()).filter(item => item.kind === 'verification').length, 1);
});

test('distinct supervised assignments can reuse one worker session and keep separate verdicts', async t => {
  const { options, workflow, receipt } = await setup(t);
  options.maxTurns = 3;
  const outcomes = await withSession(options, async agent => [
    await supervisedWorkflow(agent, workflow),
    await supervisedWorkflow(agent, { ...workflow, id: 'related-parser-case',
      verificationFile: join(options.artifactDir, 'related-verification.jsonl') }),
  ]);
  assert.deepEqual(outcomes.map(outcome => outcome.attempts), [2, 1]);
  assert.ok(outcomes.every(outcome => outcome.accepted));
  const evidence = await receipt();
  assert.deepEqual(evidence.turns.map(turn => turn.id), ['parser-1', 'parser-2', 'related-parser-case-1']);
  assert.equal(new Set(evidence.turns.map(turn => turn.native_session_id)).size, 1);
  assert.equal(evidence.processes.length, 1);
});

test('exhausted revision budget remains unaccepted despite successful worker turns', async t => {
  const { options, workflow, calls, records } = await setup(t, 'never-fix');
  const outcome = await withSession(options, agent => supervisedWorkflow(agent, { ...workflow, maxRevisions: 1 }));
  assert.equal(outcome.status, 'verification_failed');
  assert.equal(outcome.accepted, false);
  assert.equal(outcome.result.status, 'succeeded');
  assert.equal(outcome.attempts, 2);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 2);
  assert.equal((await records()).at(-1).accepted, false);
});

test('unsuccessful worker execution stops before verification or correction', async t => {
  const { options, workflow, calls, records } = await setup(t, 'worker-failed');
  const outcome = await withSession(options, agent => supervisedWorkflow(agent, {
    ...workflow, verify() { throw new Error('must not verify a failed execution'); },
  }));
  assert.equal(outcome.status, 'worker_failed');
  assert.equal(outcome.accepted, false);
  assert.equal(outcome.verification, null);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 1);
  assert.ok(!(await records()).some(item => item.kind === 'verification_started'));
});

test('caller checkpoint can stop a proposed revision without another worker turn', async t => {
  const { options, workflow, calls, records } = await setup(t);
  const checkpoints = [];
  const outcome = await withSession(options, agent => supervisedWorkflow(agent, {
    ...workflow,
    checkpoint({ attempt, verification }) { checkpoints.push({ attempt, verification }); return attempt === 1; },
  }));
  assert.equal(outcome.status, 'stopped');
  assert.equal(outcome.accepted, false);
  assert.equal(outcome.attempts, 1);
  assert.equal(checkpoints[0].verification, null);
  assert.equal(checkpoints[1].verification.accepted, false);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 1);
  assert.equal((await records()).at(-2).proceed, false);
});

test('an initial checkpoint refusal does not start worker execution', async t => {
  const { options, workflow, calls } = await setup(t);
  const outcome = await withSession(options, agent => supervisedWorkflow(agent, {
    ...workflow, checkpoint: () => false,
    verify() { throw new Error('must not verify before a turn'); },
  }));
  assert.equal(outcome.status, 'stopped');
  assert.equal(outcome.accepted, false);
  assert.equal(outcome.attempts, 0);
  assert.equal(outcome.result, null);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 0);
});

test('verifier exception is saved and never retried', async t => {
  const { cwd, options, workflow, calls, records, receipt } = await setup(t);
  await assert.rejects(withSession(options, agent => supervisedWorkflow(agent, {
    ...workflow, verify() { throw new Error('independent verifier unavailable'); },
  })), /independent verifier unavailable/);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 1);
  assert.equal((await records()).at(-1).error, 'independent verifier unavailable');
  assert.match((await receipt()).body_error, /independent verifier unavailable/);
  assert.equal(await readFile(join(cwd, 'exited'), 'utf8'), 'yes');
});

test('invalid verifier acceptance cannot become success or another worker turn', async t => {
  const { options, workflow, calls } = await setup(t);
  await assert.rejects(withSession(options, agent => supervisedWorkflow(agent, {
    ...workflow, verify() { return { accepted: 'yes', feedback: 'claimed success' }; },
  })), /verify must return/);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 1);
});

test('transport failure is saved and never starts a correction', async t => {
  const { options, workflow, calls, records } = await setup(t, 'transport-error');
  await assert.rejects(withSession(options, agent => supervisedWorkflow(agent, workflow)), /synthetic transport failure/);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 1);
  assert.equal((await records()).at(-1).kind, 'workflow_failed');
});

test('parent cancellation during verification awaits cooperative cleanup and sends no correction', { timeout: 10000 }, async t => {
  const { cwd, options, workflow, calls, records } = await setup(t);
  const controller = new AbortController();
  options.signal = controller.signal;
  let settled = false;
  await assert.rejects(withSession(options, agent => supervisedWorkflow(agent, {
    ...workflow,
    async verify({ signal }) {
      assert.equal(signal.aborted, false);
      const stopped = new Promise(resolve => signal.addEventListener('abort', resolve, { once: true }));
      controller.abort(new Error('parent changed the assignment'));
      await stopped;
      // Simulate joining a verifier's own process after it observes cancellation.
      await new Promise(resolve => setTimeout(resolve, 20));
      settled = true;
      return { accepted: true, feedback: 'too late to accept' };
    },
  })), /parent changed the assignment/);
  assert.equal(settled, true);
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 1);
  assert.equal((await records()).at(-1).cancelled, true);
  assert.ok(!(await records()).some(item => item.kind === 'verification'));
  assert.equal(await readFile(join(cwd, 'exited'), 'utf8'), 'yes');
});

test('receipt collision and invalid limits fail before any worker turn', async t => {
  const { options, workflow, calls } = await setup(t);
  await withSession(options, async agent => {
    await assert.rejects(supervisedWorkflow(agent, { ...workflow, maxRevisions: -1 }), /maxRevisions/);
    await writeFile(workflow.verificationFile, 'existing evidence');
    await assert.rejects(supervisedWorkflow(agent, workflow), { code: 'EEXIST' });
    assert.equal(await readFile(workflow.verificationFile, 'utf8'), 'existing evidence');
  });
  assert.equal((await calls()).filter(call => call.method === 'turn.start').length, 0);
});
