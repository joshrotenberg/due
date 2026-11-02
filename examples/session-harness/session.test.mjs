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

test('unsupported backends, profiles and pre-cancelled scopes never create artifacts or launch', async t => {
  const { options } = await setup(t);
  await assert.rejects(withSession({ ...options, backend: 'resident' }, () => {}), /backend must be resume or rpc/);
  await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
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
  for (const backend of ['resume', 'rpc']) {
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
});


test('comparison CLI selects resume and RPC, and RPC example consumes complete items', async t => {
  const { dir, options } = await setup(t);
  await copyFile(fileURLToPath(new URL('./session.fixture.mjs', import.meta.url)), options.dueBinary);
  await chmod(options.dueBinary, 0o700);
  const run = promisify(execFile);
  const common = ['--due', options.dueBinary, '--provider', 'codex', '--cwd', dir];
  for (const selection of [[], ['--backend', 'both'], ['--backend', 'all']]) {
    const comparison = await run(process.execPath, [fileURLToPath(new URL('./compare.mjs', import.meta.url)),
      ...common, ...selection, '--artifacts', join(dir, `comparison-${selection.at(-1) ?? 'default'}`)], { timeout: 10000 });
    const summary = JSON.parse(comparison.stdout);
    assert.deepEqual(summary.arms.map(arm => arm.backend), ['resume', 'rpc']);
    assert.deepEqual(summary.arms.map(arm => arm.processes.length), [2, 1]);
  }
  const rejected = join(dir, 'removed-backend');
  await assert.rejects(run(process.execPath, [fileURLToPath(new URL('./compare.mjs', import.meta.url)),
    ...common, '--backend', 'resident', '--artifacts', rejected]), /usage:/);
  await assert.rejects(stat(rejected), { code: 'ENOENT' });
  const example = await run(process.execPath, [fileURLToPath(new URL('./rpc-example.mjs', import.meta.url)),
    ...common, '--artifacts', join(dir, 'rpc-example')], { timeout: 10000 });
  const records = example.stdout.trim().split('\n').map(JSON.parse);
  assert.equal(records.filter(record => record.data.kind === 'turn_finished').length, 2);
  assert.equal(records.at(-1).data.kind, 'session_closed');
  assert.deepEqual(records.find(record => record.data.kind === 'native').data.data.params.item.unknown.nested,
    [null, 7, { keep: true }]);
  assert.match(example.stderr, /Validated both turns/);
});

{
  const backend = 'rpc';
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

test('onInteraction is validated and rejected for resume before anything is created or launched', async t => {
  const { options } = await setup(t);
  await assert.rejects(withSession({ ...options, onInteraction: () => ({}) }, () => {}), /unsupported by the resume backend/);
  await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
  for (const backend of ['resume', 'rpc']) {
    for (const bad of ['allow', {}, null]) {
      await assert.rejects(withSession({ ...options, backend, onInteraction: bad }, () => {}), /onInteraction must be a function/);
    }
  }
  await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
});

test('turnWaitMs is validated before scope artifacts or launch and is unsupported for resume', async t => {
  const { dir, options } = await setup(t);
  options.dueBinary = join(dir, 'must-not-launch');
  {
    const backend = 'rpc';
    for (const turnWaitMs of [0, -1, 1.5, NaN, Infinity, 2 ** 31, Number.MAX_SAFE_INTEGER, '20', false, {}, []]) {
      await assert.rejects(withSession({ ...options, backend, turnWaitMs }, () => {}), /turnWaitMs/);
      await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
    }
  }
  for (const turnWaitMs of [null, 20, 2 ** 31 - 1, 0, NaN]) {
    await assert.rejects(withSession({ ...options, turnWaitMs }, () => {}), /unsupported.*resume/);
    await assert.rejects(stat(options.artifactDir), { code: 'ENOENT' });
  }
});
test('an undefined turn wait keeps the resume backend and its receipt unchanged', async t => {
  const { options, receipt } = await setup(t, { turnWaitMs: undefined });
  await withSession(options, agent => agent.turn({ id: 'first', prompt: 'remember' }));
  assert.equal(Object.hasOwn(await receipt(), 'turn_wait_ms'), false);
});

{
  const backend = 'rpc';
  async function interactive(t, model, extra = {}) {
    const ctx = await setup(t, { backend, ...extra });
    ctx.options.profile.model = model;
    await copyFile(fileURLToPath(new URL('./resident.fixture.mjs', import.meta.url)), ctx.options.dueBinary);
    return ctx;
  }
  test(`${backend}: turn wait defaults and supported integer bounds are recorded without native forwarding`, async t => {
    for (const [extra, expected] of [[{}, 300_000], [{ turnWaitMs: undefined }, 300_000], [{ turnWaitMs: 1 }, 1], [{ turnWaitMs: 2 ** 31 - 1 }, 2 ** 31 - 1]]) {
      const { dir, options, receipt } = await interactive(t, 'early', extra);
      options.profile.timeout = '2m';
      await withSession(options, agent => agent.turn({ id: 'first', prompt: 'one' }));
      const evidence = await receipt();
      assert.equal(evidence.turn_wait_ms, expected);
      assert.equal(evidence.profile.timeout, '2m');
      const input = (await readFile(join(dir, 'fixture-input.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
      const opening = input.find(row => row.method === 'session.open');
      const nativeProfile = opening.params;
      assert.equal(nativeProfile.timeout, '2m');
      assert.equal(Object.hasOwn(nativeProfile, 'turnWaitMs'), false);
      assert.equal(Object.hasOwn(nativeProfile, 'turn_wait_ms'), false);
      assert.equal(evidence.body_error, null);
    }
  });
  test(`${backend}: a short turn wait retains the caller failure and clean process receipt`, { timeout: 5000 }, async t => {
    const controller = new AbortController();
    const fuse = setTimeout(() => controller.abort(new Error('test failsafe')), 3000);
    t.after(() => clearTimeout(fuse));
    const { dir, options, receipt } = await interactive(t, 'hold', { turnWaitMs: 20, signal: controller.signal });
    await assert.rejects(withSession(options, agent => agent.turn({ id: 'held', prompt: 'wait' })), /timed out waiting for its terminal item/);
    const evidence = await receipt();
    assert.equal(evidence.turn_wait_ms, 20);
    assert.match(evidence.body_error, /timed out waiting for its terminal item/);
    assert.equal(evidence.cleanup_error, null);
    assert.equal(evidence.native_session_id, 'native-one');
    assert.equal(evidence.processes.length, 1);
    assert.equal(evidence.processes[0].exit_code, 0);
    assert.equal(evidence.processes[0].forced_kill, false);
    assert.equal(await readFile(join(dir, 'fixture-exited'), 'utf8'), 'closed\n');
  });
  test(`${backend}: a disabled turn wait completes delayed turns in the same process and session`, { timeout: 5000 }, async t => {
    const { options, receipt } = await interactive(t, 'wait-delayed', { turnWaitMs: null });
    const outcomes = await withSession(options, async agent => [
      await agent.turn({ id: 'first', prompt: 'remember' }),
      await agent.turn({ id: 'second', prompt: 'recall' }),
    ]);
    assert.deepEqual(outcomes.map(result => result.text), ['turn 1', 'turn 2']);
    assert.deepEqual(outcomes.map(result => result.session_id), ['native-one', 'native-one']);
    const evidence = await receipt();
    assert.equal(evidence.turn_wait_ms, null);
    assert.equal(evidence.turns.length, 2);
    assert.ok(evidence.turns.every(turn => turn.duration_ms >= 100));
    assert.equal(evidence.native_session_id, 'native-one');
    assert.equal(evidence.processes.length, 1);
    assert.ok(evidence.processes[0].pid > 0);
    assert.equal(evidence.processes[0].exit_code, 0);
    assert.equal(evidence.processes[0].forced_kill, false);
    assert.equal(evidence.body_error, null);
    assert.equal(evidence.cleanup_error, null);
  });
  test(`${backend}: caller cancellation still settles a turn with its wait disabled`, { timeout: 5000 }, async t => {
    const controller = new AbortController();
    let ready;
    const started = new Promise(resolve => { ready = resolve; });
    const { dir, options, receipt } = await interactive(t, 'hold', { turnWaitMs: null, signal: controller.signal,
      onItem: record => { if (record.data.kind === 'turn_started') ready(); } });
    await assert.rejects(withSession(options, async agent => {
      const turn = agent.turn({ id: 'held', prompt: 'wait' });
      await started;
      await new Promise(resolve => setImmediate(resolve));
      controller.abort(new Error('caller cancelled unlimited wait'));
      return turn;
    }), /cancelled/);
    const evidence = await receipt();
    assert.equal(evidence.turn_wait_ms, null);
    assert.match(evidence.body_error, /cancelled/);
    assert.equal(evidence.cleanup_error, null);
    assert.equal(evidence.processes[0].exit_code, 0);
    assert.equal(evidence.processes[0].forced_kill, false);
    assert.equal(await readFile(join(dir, 'fixture-exited'), 'utf8'), 'closed\n');
  });
  test(`${backend}: interactionReply capability is true only with a callback`, async t => {
    const without = await interactive(t, 'early');
    assert.equal(await withSession(without.options, async agent => agent.capabilities.interactionReply), false);
    const withCallback = await interactive(t, 'early', { onInteraction: () => ({}) });
    assert.equal(await withSession(withCallback.options, async agent => agent.capabilities.interactionReply), true);
    assert.equal((await withCallback.receipt()).capabilities.interactionReply, true);
  });
  test(`${backend}: default stays fail-closed with receipts`, async t => {
    const { options, receipt } = await interactive(t, 'interaction');
    await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'x' })), /explicit decision/);
    const evidence = await receipt();
    assert.match(evidence.body_error, /explicit decision/);
    assert.equal(evidence.processes[0].exit_code, 0);
  });
  test(`${backend}: async decision, onItem sees the raw item and the reply carries exact IDs`, async t => {
    const seen = [];
    const { options, receipt } = await interactive(t, 'interaction', {
      onItem: record => seen.push(record.data),
      onInteraction: async (record, signal) => {
        assert.ok(signal instanceof AbortSignal);
        await new Promise(resolve => setTimeout(resolve, 20));
        return { decision: 'decline', for: [record.data.session_id, record.data.turn_id, record.data.data.request_id] };
      } });
    const result = await withSession(options, agent => agent.turn({ id: 'first', prompt: 'x' }));
    assert.equal(result.status, 'succeeded');
    assert.deepEqual(seen.find(item => item.kind === 'interaction').data, { request_id: 'permission', request: { command: 'must-not-run' } });
    const reply = seen.find(item => item.data?.method === 'fixture/reply');
    assert.deepEqual(reply.data.params, { decision: 'decline', for: ['due-session', 'native-turn-1', 'permission'] });
    const evidence = await receipt();
    assert.equal(evidence.body_error, null);
    assert.equal(evidence.processes[0].exit_code, 0);
    const saved = (await readFile(join(options.artifactDir, 'items.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
    assert.ok(saved.some(row => row.data.kind === 'interaction'));
  });
  test(`${backend}: a callback error is the caller failure and process receipts survive`, async t => {
    const original = new Error('policy exploded');
    const { options, receipt } = await interactive(t, 'interaction', { onInteraction: () => { throw original; } });
    await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'x' })), error => error === original);
    const evidence = await receipt();
    assert.match(evidence.body_error, /policy exploded/);
    assert.equal(evidence.native_session_id, 'native-one');
    assert.equal(evidence.processes.length, 1);
    assert.ok(evidence.processes[0].pid > 0); assert.equal(evidence.processes[0].exit_code, 0);
    assert.equal(evidence.cleanup_error, null);
  });
  test(`${backend}: a genuine reply failure during pending close is the body failure, not cleanup`, { timeout: 10000 }, async t => {
    const { options, receipt } = await interactive(t, 'interaction-late-error-on-close', { onInteraction: () => ({ decision: 'decline' }) });
    await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'x' })), /permissions backend unavailable/);
    const evidence = await receipt();
    assert.match(evidence.body_error, /permissions backend unavailable/);
    assert.equal(evidence.cleanup_error, null);
    assert.equal(evidence.processes[0].exit_code, 0);
    assert.equal(evidence.processes[0].signal, null);
    assert.equal(evidence.native_session_id, 'native-one');
  });
  test(`${backend}: a callback rejecting after the scope closed is handled, unsent and cannot rewrite the receipt`, { timeout: 10000 }, async t => {
    const controller = new AbortController();
    let reject;
    const gate = new Promise((_, no) => { reject = no; });
    const { dir, options, receipt } = await interactive(t, 'interaction', { signal: controller.signal,
      onInteraction: async () => { controller.abort(new Error('caller cancelled')); await gate; } });
    const unhandled = [];
    const listener = reason => unhandled.push(reason);
    process.on('unhandledRejection', listener);
    t.after(() => process.off('unhandledRejection', listener));
    await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'x' })), /cancelled/);
    const before = await readFile(join(options.artifactDir, 'session.json'), 'utf8');
    reject(new Error('arrived after the scope closed'));
    await new Promise(resolve => setTimeout(resolve, 100));
    assert.deepEqual(unhandled, []);
    assert.equal(await readFile(join(options.artifactDir, 'session.json'), 'utf8'), before);
    assert.ok(!/interaction[._]reply/.test(await readFile(join(dir, 'fixture-input.jsonl'), 'utf8')));
    assert.doesNotMatch(before, /arrived after/);
    assert.equal((await receipt()).cleanup_error, null);
  });
  test(`${backend}: a fresh null-open session keeps the root init native ID through observer and protocol failures`, async t => {
    for (const [model, observer, pattern] of [['init-hold', true, /observer failed/], ['init-malformed', false, /JSON/]]) {
      const { options, receipt } = await interactive(t, model);
      if (observer) options.onItem = record => { if (record.data.data?.event?.session_id === 'claude-native') throw new Error('observer failed after init'); };
      await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'x' })), pattern);
      const evidence = await receipt();
      assert.equal(evidence.native_session_id, 'claude-native');
      assert.equal(evidence.processes[0].exit_code, 0); assert.equal(evidence.cleanup_error, null);
      assert.ok(evidence.body_error);
    }
  });
  test(`${backend}: scope cancellation does not hang on an uncooperative callback or send late`, { timeout: 10000 }, async t => {
    const controller = new AbortController();
    let release;
    const gate = new Promise(resolve => { release = resolve; });
    const { dir, options, receipt } = await interactive(t, 'interaction', { signal: controller.signal,
      onInteraction: async () => { controller.abort(new Error('caller cancelled')); await gate; return { decision: 'accept' }; } });
    await assert.rejects(withSession(options, agent => agent.turn({ id: 'first', prompt: 'x' })), /caller cancelled|cancelled/);
    release();
    await new Promise(resolve => setTimeout(resolve, 50));
    const input = await readFile(join(dir, 'fixture-input.jsonl'), 'utf8');
    assert.ok(!/interaction[._]reply/.test(input));
    assert.equal((await receipt()).processes[0].exit_code, 0);
  });
}
