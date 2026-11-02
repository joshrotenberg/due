import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { chmod, copyFile, mkdtemp, readFile, realpath, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { withSession } from './session.mjs';
import { correctionWorkflow } from './correction.mjs';

async function setup(t, overrides = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'due-session-scope-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const dueBinary = join(dir, 'due');
  await copyFile(fileURLToPath(new URL('./fake-resume-due.mjs', import.meta.url)), dueBinary);
  await chmod(dueBinary, 0o700);
  const options = { backend: 'resume', dueBinary, artifactDir: join(dir, 'artifacts'),
    profile: { provider: 'codex', cwd: dir }, ...overrides };
  return { dir, options, receipt: async () => JSON.parse(await readFile(join(options.artifactDir, 'session.json'), 'utf8')) };
}

async function waitFor(path) {
  const deadline = Date.now() + 3000;
  while (Date.now() < deadline) {
    try { await stat(path); return; } catch {}
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  throw new Error(`Fixture did not create ${path}`);
}

test('scope returns raw turn results, preserves profile and rejects escaped handles', async t => {
  const { dir, options, receipt } = await setup(t);
  let escaped;
  const result = await withSession(options, async agent => {
    escaped = agent;
    const first = await agent.turn({ id: 'first', prompt: 'remember' });
    options.profile.cwd = '/caller-mutated-after-open';
    assert.equal(first.status, 'succeeded');
    return agent.turn({ id: 'second', prompt: 'recall' });
  });
  assert.equal(result.text, 'remembered');
  await assert.rejects(escaped.turn({ id: 'escaped', prompt: 'no' }), /scope is closed/);
  const evidence = await receipt();
  assert.equal(evidence.turns.length, 2);
  assert.equal(evidence.processes.length, 2);
  assert.equal(evidence.native_session_id, 'fixture-native');
  assert.equal(evidence.profile.cwd, await realpath(dir));
  assert.ok(evidence.events.count >= 2);
  assert.notEqual(evidence.turns[0].process.pid, evidence.turns[1].process.pid);
});

test('unsupported profiles and pre-cancelled scopes never create artifacts or launch', async t => {
  const { options } = await setup(t);
  await assert.rejects(withSession({ ...options, profile: { ...options.profile, native_args: ['--bypass'] } }, () => {}), /unsupported shared profile field/);
  await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
  await assert.rejects(withSession({ ...options, profile: { ...options.profile, cwd: options.dueBinary } }, () => {}), /cwd must be a directory/);
  await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
  const controller = new AbortController(); controller.abort(new Error('before open'));
  await assert.rejects(withSession({ ...options, signal: controller.signal }, () => {}), /before open/);
  await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
});

test('overlap, duplicate IDs and turn budget do not admit extra processes', async t => {
  const { options, receipt } = await setup(t, { maxTurns: 2 });
  await withSession(options, async agent => {
    const first = agent.turn({ id: 'first', prompt: 'remember' });
    await assert.rejects(agent.turn({ id: 'overlap', prompt: 'no' }), /one turn at a time/);
    await first;
    await assert.rejects(agent.turn({ id: 'first', prompt: 'no' }), /duplicate/);
    await agent.turn({ id: 'second', prompt: 'recall' });
    await assert.rejects(agent.turn({ id: 'third', prompt: 'no' }), /maxTurns/);
  });
  assert.equal((await receipt()).processes.length, 2);
});

test('returning with live work fails the scope and joins that process', { timeout: 10000 }, async t => {
  const { dir, options, receipt } = await setup(t);
  await assert.rejects(withSession(options, async agent => {
    agent.turn({ id: 'held', prompt: 'hold' });
    await waitFor(join(dir, 'ready'));
    return 'forgot to await';
  }), /active turn/);
  const evidence = await receipt();
  assert.match(evidence.body_error, /active turn/);
  assert.equal(evidence.processes[0].exit_code, 130);
  assert.equal(await readFile(join(dir, 'cancelled'), 'utf8'), 'yes');
  assert.throws(() => process.kill(evidence.processes[0].pid, 0), { code: 'ESRCH' });
});

test('observer failure is retained and cannot produce a successful scope', async t => {
  const { options, receipt } = await setup(t, { onItem() { throw new Error('observer failed'); } });
  await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'remember' })), /observer failed/);
  const evidence = await receipt();
  assert.match(evidence.body_error, /observer failed/);
  assert.ok(evidence.events.count >= 1);
  assert.ok(evidence.processes[0].exit_code !== null || evidence.processes[0].signal !== null);
});

test('parent cancellation cannot become scope success when a body accepts its cancelled result', { timeout: 10000 }, async t => {
  const controller = new AbortController();
  const { dir, options, receipt } = await setup(t, { signal: controller.signal });
  await assert.rejects(withSession(options, async agent => {
    const held = agent.turn({ id: 'held', prompt: 'hold' });
    await waitFor(join(dir, 'ready'));
    controller.abort(new Error('caller cancelled'));
    assert.equal((await held).status, 'cancelled');
    return 'handled';
  }), /caller cancelled/);
  assert.match((await receipt()).body_error, /caller cancelled/);
});

test('falsy JavaScript thrown values remain scope failures', async t => {
  for (const value of [undefined, null, false, 0, '']) {
    await t.test(String(value), async t => {
      const { options, receipt } = await setup(t);
      await assert.rejects(withSession(options, () => { throw value; }));
      assert.equal((await receipt()).body_error, String(value));
    });
  }
});

test('ordinary failed execution remains inspectable data', async t => {
  const { options, receipt } = await setup(t);
  const result = await withSession(options, agent => agent.turn({ id: 'failure', prompt: 'fail' }));
  assert.equal(result.status, 'failed');
  assert.equal((await receipt()).body_error, null);
  assert.equal((await receipt()).turns[0].process.exit_code, 1);
});

test('identical correction workflow validates through all backends with distinct process ownership', async t => {
  const outputs = [];
  for (const backend of ['resume', 'resident', 'rpc']) {
    await t.test(backend, async t => {
      const { dir, options, receipt } = await setup(t, { backend });
      await copyFile(fileURLToPath(new URL('./session.fixture.mjs', import.meta.url)), options.dueBinary);
      await chmod(options.dueBinary, 0o700);
      const output = await withSession(options, correctionWorkflow);
      outputs.push(output);
      assert.deepEqual(output, { draft: { marker: 'larch-4821', revision: 1, total: 19 },
        corrected: { marker: 'larch-4821', revision: 2, total: 17 } });
      const evidence = await receipt();
      const nativeIds = new Set(evidence.turns.map(turn => turn.native_session_id));
      assert.equal(nativeIds.size, 1);
      assert.equal(evidence.processes.length, backend === 'resume' ? 2 : 1);
      assert.ok(evidence.processes.every(process => process.exit_code === 0));
      assert.ok(evidence.turns.every(turn => backend === 'resume' ? turn.process !== null : turn.process === null));
      const launches = (await readFile(join(dir, 'launches.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
      assert.equal(launches.length, backend === 'resume' ? 2 : 1);
      if (backend === 'resume') assert.equal(launches[1].resume, launches[0].nativeId);
      else {
        const records = (await readFile(join(options.artifactDir, 'items.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
        assert.equal(records.at(-1).data.kind, 'session_closed');
        assert.deepEqual(records.find(record => record.data.kind === 'native').data.data.params.item.unknown.nested, [null, 7, { keep: true }]);
      }
    });
  }
  assert.deepEqual(outputs[0], outputs[1]);
  assert.deepEqual(outputs[1], outputs[2]);
});


test('comparison CLI accepts all transports and RPC example consumes complete items', async t => {
  const { dir, options } = await setup(t);
  await copyFile(fileURLToPath(new URL('./session.fixture.mjs', import.meta.url)), options.dueBinary);
  await chmod(options.dueBinary, 0o700);
  const run = promisify(execFile);
  const common = ['--due', options.dueBinary, '--provider', 'codex', '--cwd', dir];
  const comparison = await run(process.execPath, [fileURLToPath(new URL('./compare.mjs', import.meta.url)),
    ...common, '--backend', 'all', '--artifacts', join(dir, 'comparison')], { timeout: 10000 });
  const summary = JSON.parse(comparison.stdout);
  assert.deepEqual(summary.arms.map(arm => arm.backend), ['resume', 'resident', 'rpc']);
  assert.deepEqual(summary.arms.map(arm => arm.processes.length), [2, 1, 1]);
  const example = await run(process.execPath, [fileURLToPath(new URL('./rpc-example.mjs', import.meta.url)),
    ...common, '--artifacts', join(dir, 'rpc-example')], { timeout: 10000 });
  const records = example.stdout.trim().split('\n').map(JSON.parse);
  assert.equal(records.filter(record => record.data.kind === 'turn_finished').length, 2);
  assert.equal(records.at(-1).data.kind, 'session_closed');
  assert.deepEqual(records.find(record => record.data.kind === 'native').data.data.params.item.unknown.nested,
    [null, 7, { keep: true }]);
  assert.match(example.stderr, /Validated both turns/);
});

for (const backend of ['resident', 'rpc']) {
  for (const mode of ['observer', 'malformed', 'cleanup-failure']) {
    test(`${backend} retains failed scope evidence: ${mode}`, async t => {
      const original = new Error('observer exploded');
      const { options, receipt } = await setup(t, { backend });
      options.profile.model = mode;
      await copyFile(fileURLToPath(new URL('./resident.fixture.mjs', import.meta.url)), options.dueBinary);
      options.onItem = record => {
        if (mode !== 'malformed' && record.data.kind === 'turn_started') throw original;
      };
      await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'test' })), error => {
        if (mode === 'observer') assert.equal(error, original);
        if (mode === 'cleanup-failure') {
          assert.ok(error instanceof AggregateError);
          assert.equal(error.errors[0], original);
          assert.match(error.errors[1].message, /exited 7/);
        }
        return true;
      });
      const evidence = await receipt();
      const exit = JSON.parse(await readFile(join(options.artifactDir, 'resident-exit.json'), 'utf8'));
      assert.equal(evidence.native_session_id, 'native-one');
      assert.equal(evidence.processes.length, 1);
      assert.ok(evidence.processes[0].pid > 0);
      assert.equal(evidence.processes[0].pid, exit.pid);
      assert.equal(evidence.processes[0].exit_code, mode === 'cleanup-failure' ? 7 : 0);
      assert.equal(evidence.processes[0].signal, null);
      assert.ok(evidence.body_error);
      if (mode === 'cleanup-failure') assert.match(evidence.cleanup_error, /exited 7/);
      else assert.equal(evidence.cleanup_error, null);
    });
  }
}
