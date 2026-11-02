import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { openRpc } from './resident.mjs';
const dueBinary = fileURLToPath(new URL('./resident.fixture.mjs', import.meta.url));
async function setup(t, model = 'early', signal, extra = {}) {
  const artifactDir = await mkdtemp(join(tmpdir(), 'due-resident-'));
  t.after(() => rm(artifactDir, { recursive: true, force: true }));
  const items = [];
  const backend = await openRpc({ dueBinary, artifactDir, profile: { provider: 'codex', cwd: artifactDir, model }, onItem: item => items.push(item), signal, ...extra });
  t.after(async () => { try { await backend.close(); } catch {} });
  return { backend, items, artifactDir };
}
test(`rpc: invalid turn waits fail before opening evidence files or launching`, async t => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'due-resident-wait-invalid-'));
  t.after(() => rm(artifactDir, { recursive: true, force: true }));
  for (const turnWaitMs of [0, -1, 1.5, NaN, Infinity, 2 ** 31, Number.MAX_SAFE_INTEGER, '20', false, {}, []]) {
    await assert.rejects(openRpc({ dueBinary: join(artifactDir, 'must-not-launch'), artifactDir,
      profile: { provider: 'codex', cwd: artifactDir, model: 'early' }, turnWaitMs }), /turnWaitMs/);
    assert.deepEqual(await readdir(artifactDir), []);
  }
});
test(`rpc: a direct caller's short turn wait fails and settles its held process`, { timeout: 5000 }, async t => {
  const control = new AbortController();
  const fuse = setTimeout(() => control.abort(new Error('test failsafe')), 3000);
  t.after(() => clearTimeout(fuse));
  const { backend, artifactDir } = await setup(t, 'hold', control.signal, { turnWaitMs: 20 });
  await assert.rejects(backend.turn({ id: 'held', prompt: 'wait' }), /timed out waiting for its terminal item/);
  const receipt = await backend.close();
  assert.match(receipt.execution_error.message, /timed out waiting for its terminal item/);
  assert.equal(receipt.native_session_id, 'native-one');
  assert.equal(receipt.processes[0].exit_code, 0);
  assert.equal(receipt.processes[0].forced_kill, false);
  assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
});
test(`rpc: two turns share a process, early results and final output are retained`, async t => {
  const { backend, items, artifactDir } = await setup(t);
  const a = await backend.turn({ id: 'first', prompt: 'one' });
  const b = await backend.turn({ id: 'second', prompt: 'two' });
  assert.equal(a.native_session_id, b.native_session_id);
  assert.equal(a.result.text, 'turn 1'); assert.equal(b.result.text, 'turn 2');
  assert.equal(a.process, null);
  const receipt = await backend.close();
  assert.equal(receipt.processes.length, 1); assert.equal(receipt.processes[0].exit_code, 0);
  assert.equal(items.at(-1).data.kind, 'session_closed');
  assert.deepEqual(items.find(i => i.data.kind === 'native').data.data.params.item.nested.future, [null, 3, true]);
  assert.deepEqual(items.filter(i => i.data.kind === 'turn_finished').map(i => i.turn_id), ['first', 'second']);
  const input = (await readFile(join(artifactDir, 'fixture-input.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
  const names = input.map(i => i.method);
  assert.equal(names.filter(name => name === 'session.open').length, 1);
  assert.equal(names.filter(name => name === 'turn.read').length, 0);
  const wire = (await readFile(join(artifactDir, 'protocol.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
  assert.ok(wire.filter(row => row.direction === 'in').every(row => !['initialize', 'notifications/initialized', 'tools/call'].includes(row.message.method)));
  assert.ok(wire.some(row => row.direction === 'out' && row.message.method === 'session.item'));
  assert.equal((JSON.parse(await readFile(join(artifactDir, 'first/result.json')))).run_id, 'native-turn-1');
});
test(`rpc: cursor gaps recover full items once`, async t => {
  const { backend, items } = await setup(t, 'gap');
  await backend.turn({ id: 'first', prompt: 'one' }); await backend.close();
  assert.deepEqual(items.map(i => i.data.cursor), [1, 2, 3, 4, 5]);
  assert.equal(items.filter(i => i.data.kind === 'native').length, 1);
});
test(`rpc: native failures are data and preserve the resident process`, async t => {
  const { backend } = await setup(t, 'failed');
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'failed');
  assert.equal((await backend.turn({ id: 'second', prompt: 'two' })).result.status, 'failed');
  assert.equal((await backend.close()).processes[0].exit_code, 0);
});
for (const [mode, pattern] of [['lost', /journal lost/], ['malformed', /JSON/], ['interaction', /explicit decision/]]) {
  test(`rpc: ${mode} errors stay explicit and the process is settled`, async t => {
    const { backend, artifactDir } = await setup(t, mode);
    await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), pattern);
    assert.match((await backend.close()).execution_error.message, pattern);
    assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
    assert.equal(JSON.parse(await readFile(join(artifactDir, 'resident-exit.json'))).exit_code, 0);
  });
}
const inputs = async artifactDir => (await readFile(join(artifactDir, 'fixture-input.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse)
  .map(row => ({ name: row.method, args: row.params }))
  .filter(row => row.name === 'interaction.reply');
async function interactive(t, model, onInteraction, signal, observe = () => {}) {
  const artifactDir = await mkdtemp(join(tmpdir(), 'due-resident-interaction-'));
  t.after(() => rm(artifactDir, { recursive: true, force: true }));
  const items = [];
  const backend = await openRpc({ dueBinary, artifactDir, profile: { provider: 'codex', cwd: artifactDir, model }, onInteraction, signal, onItem: item => { items.push(item); observe(item); } });
  t.after(async () => { try { await backend.close(); } catch {} });
  return { backend, items, artifactDir };
}
test(`rpc: interactionReply is advertised only with a callback`, async t => {
  assert.equal((await setup(t)).backend.capabilities.interactionReply, false);
  assert.equal((await interactive(t, 'early', () => ({}))).backend.capabilities.interactionReply, true);
  await assert.rejects(openRpc({ dueBinary, artifactDir: '/nonexistent', profile: {}, onInteraction: 'yes' }), /must be a function/);
});
for (const response of [{ decision: 'decline' }, { decision: 'accept' }]) {
  test(`rpc: async ${response.decision} is routed with exact IDs while items keep flowing`, async t => {
    let seenAtDecision, passed;
    const { backend, items, artifactDir } = await interactive(t, 'interaction', async (record, signal) => {
      passed = { record, signal };
      await new Promise(resolve => setTimeout(resolve, 50));
      seenAtDecision = items.map(i => i.data.kind + ':' + (i.data.data.method ?? ''));
      return response;
    });
    const turn = await backend.turn({ id: 'first', prompt: 'one' });
    assert.equal(turn.result.status, 'succeeded');
    assert.ok(seenAtDecision.includes('native:fixture/during-decision'));
    assert.equal(passed.record.data.kind, 'interaction'); assert.equal(passed.record.turn_id, 'first');
    assert.ok(passed.signal instanceof AbortSignal);
    const replies = await inputs(artifactDir);
    assert.deepEqual(replies, [{ name: 'interaction.reply', args: { session_id: 'due-session', turn_id: 'native-turn-1', request_id: 'permission', response } }]);
    assert.deepEqual(items.find(i => i.data.data.method === 'fixture/reply').data.data.params, response);
    assert.equal((await backend.close()).processes[0].exit_code, 0);
  });
}
for (const [label, callback, pattern] of [
  ['callback error', () => { throw new Error('policy exploded'); }, /policy exploded/],
  ['async callback error', async () => { throw new Error('policy rejected'); }, /policy rejected/],
  ['invalid response', () => 'accept', /provider-native response object/],
  ['missing response', () => undefined, /provider-native response object/]]) {
  test(`rpc: ${label} fails truthfully without sending a reply`, async t => {
    const { backend, artifactDir } = await interactive(t, 'interaction', callback);
    await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), pattern);
    assert.match((await backend.close()).execution_error.message, pattern);
    assert.deepEqual(await inputs(artifactDir), []);
    assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
  });
}
test(`rpc: cancellation signals the callback, ignores its late result and does not hang`, async t => {
  const control = new AbortController();
  let release, observed;
  const gate = new Promise(resolve => { release = resolve; });
  const { backend, artifactDir } = await interactive(t, 'interaction', async (record, signal) => {
    observed = signal; await gate; return { decision: 'accept' }; // Deliberately ignores the signal.
  }, control.signal);
  const turn = backend.turn({ id: 'first', prompt: 'one' });
  while (!observed) await new Promise(resolve => setTimeout(resolve, 5));
  control.abort();
  await assert.rejects(turn, /cancelled/);
  assert.equal(observed.aborted, true);
  const receipt = await backend.close(); // The callback is still pending here.
  release();
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.match(receipt.execution_error.message, /cancelled/);
  assert.deepEqual(await inputs(artifactDir), []);
  assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
});
test(`rpc: closing aborts a pending decision and never sends it late`, async t => {
  let release, observed;
  const gate = new Promise(resolve => { release = resolve; });
  const { backend, artifactDir } = await interactive(t, 'interaction', async (record, signal) => { observed = signal; await gate; return { decision: 'accept' }; });
  const turn = backend.turn({ id: 'first', prompt: 'one' }); turn.catch(() => {});
  while (!observed) await new Promise(resolve => setTimeout(resolve, 5));
  const started = performance.now();
  const receipt = await backend.close();
  assert.ok(performance.now() - started < 5000);
  assert.equal(receipt.execution_error, undefined); // Releasing the waiter is not a failure.
  assert.equal(observed.aborted, true);
  release();
  await assert.rejects(turn, /closed before the turn finished/);
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.deepEqual(await inputs(artifactDir), []);
});
test(`rpc: a turn finishing first makes the decision stale and unsent`, async t => {
  let release, observed;
  const gate = new Promise(resolve => { release = resolve; });
  const { backend, artifactDir } = await interactive(t, 'interaction-finished', async (record, signal) => { observed = signal; await gate; return { decision: 'accept' }; });
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'succeeded');
  assert.equal(observed.aborted, true);
  release();
  assert.equal((await backend.close()).execution_error, undefined);
  assert.deepEqual(await inputs(artifactDir), []);
});
test(`rpc: a reply racing turn completion is not an error`, async t => {
  const { backend, artifactDir } = await interactive(t, 'interaction-race', () => ({ decision: 'decline' }));
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'succeeded');
  const receipt = await backend.close();
  assert.equal(receipt.execution_error, undefined);
  assert.equal((await inputs(artifactDir)).length, 1);
  assert.equal(receipt.processes[0].exit_code, 0);
});
for (const [mode, pattern] of [['interaction-stale-genuine', /unknown or finished turn/], ['interaction-reject', /Fixture rejected reply/]]) {
  test(`rpc: ${mode} reply rejection is a real failure when the turn did not finish`, async t => {
    const { backend } = await interactive(t, mode, () => ({ decision: 'decline' }));
    await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), pattern);
    assert.match((await backend.close()).execution_error.message, pattern);
  });
}
test(`rpc: onInteraction gets the streamed record shape, not the process receipt`, async t => {
  let got;
  const { backend } = await interactive(t, 'interaction', record => { got = record; return { decision: 'decline' }; });
  await backend.turn({ id: 'first', prompt: 'one' });
  assert.deepEqual(Object.keys(got).sort(), ['data', 'source', 'turn_id']);
  assert.equal(got.source, 'native_session');
  assert.equal(got.data.session_id, 'due-session'); assert.equal(got.data.data.request_id, 'permission');
  assert.equal(got.pid, undefined);
});
test(`rpc: onItem mutation cannot redirect the reply or lifecycle`, async t => {
  let got;
  const { backend, artifactDir } = await interactive(t, 'interaction', record => { got = record; return { decision: 'accept' }; }, undefined, record => {
    if (record.data.kind === 'interaction') {
      Object.assign(record.data, { session_id: 'forged', turn_id: 'forged' }); record.data.data.request_id = 'forged';
    }
    if (record.data.kind === 'turn_finished') record.data.turn_id = 'forged';
  });
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'succeeded');
  assert.equal(got.data.session_id, 'due-session'); assert.equal(got.data.data.request_id, 'permission');
  assert.deepEqual((await inputs(artifactDir)).map(row => [row.args.session_id, row.args.turn_id, row.args.request_id]),
    [['due-session', 'native-turn-1', 'permission']]);
});
test(`rpc: a genuine reply error after the terminal item is preserved`, async t => {
  const { backend } = await interactive(t, 'interaction-late-error', () => ({ decision: 'decline' }));
  await backend.turn({ id: 'first', prompt: 'one' }).catch(() => {});
  for (let i = 0; i < 400 && !backend.getReceipt().execution_error; i++) await new Promise(resolve => setTimeout(resolve, 5));
  assert.match(backend.getReceipt().execution_error?.message ?? '', /permissions backend unavailable/);
  assert.match((await backend.close()).execution_error.message, /permissions backend unavailable/);
});
test(`rpc: a callback error after the turn finished is preserved; cooperative abort is not`, async t => {
  let fail;
  const { backend } = await interactive(t, 'interaction-finished', (record, signal) => new Promise((resolve, reject) => {
    signal.addEventListener('abort', () => reject(fail ? new Error('policy crashed late') : signal.reason), { once: true });
  }));
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'succeeded');
  assert.equal((await backend.close()).execution_error, undefined); // Rejecting with the abort reason is cooperative.
  fail = true;
  const second = await interactive(t, 'interaction-finished', (record, signal) => new Promise((resolve, reject) => {
    signal.addEventListener('abort', () => reject(new Error('policy crashed late')), { once: true });
  }));
  await second.backend.turn({ id: 'first', prompt: 'one' }).catch(() => {});
  for (let i = 0; i < 400 && !second.backend.getReceipt().execution_error; i++) await new Promise(resolve => setTimeout(resolve, 5));
  assert.match((await second.backend.close()).execution_error.message, /policy crashed late/);
});
test(`rpc: a matching interaction_resolved aborts the pending decision and nothing is sent late`, async t => {
  let release, observed;
  const gate = new Promise(resolve => { release = resolve; });
  const { backend, artifactDir } = await interactive(t, 'interaction-resolved', async (record, signal) => { observed = signal; await gate; return { decision: 'accept' }; });
  const turn = backend.turn({ id: 'first', prompt: 'one' });
  while (!observed?.aborted) await new Promise(resolve => setTimeout(resolve, 5));
  release(); // The callback returns while the turn is still active.
  assert.equal((await turn).result.status, 'succeeded');
  assert.deepEqual(await inputs(artifactDir), []);
  assert.equal((await backend.close()).execution_error, undefined);
});
test(`rpc: an unrelated request's resolution leaves the decision alive`, async t => {
  let observed;
  const { backend, artifactDir } = await interactive(t, 'interaction-unrelated', async (record, signal) => {
    observed = signal; await new Promise(resolve => setTimeout(resolve, 50)); return { decision: 'decline' };
  });
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'succeeded');
  assert.equal(observed.aborted, true); // Aborted only afterwards, by the turn finishing.
  assert.equal((await inputs(artifactDir)).length, 1);
  assert.equal((await backend.close()).execution_error, undefined);
});
test(`rpc: an in-flight reply losing to its own resolution is a healthy race`, async t => {
  const { backend, artifactDir } = await interactive(t, 'interaction-resolved-race', () => ({ decision: 'decline' }));
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'succeeded');
  assert.equal((await inputs(artifactDir)).length, 1);
  const receipt = await backend.close();
  assert.equal(receipt.execution_error, undefined); assert.equal(receipt.processes[0].exit_code, 0);
  const wire = (await readFile(join(artifactDir, 'protocol.jsonl'), 'utf8')).split('\n').filter(Boolean).map(JSON.parse);
  assert.ok(wire.some(row => row.direction === 'local' && row.message.event === 'stale_interaction_reply'));
});
test(`rpc: another request's resolution does not excuse a stale reply error`, async t => {
  const { backend } = await interactive(t, 'interaction-unrelated-race', () => ({ decision: 'decline' }));
  await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), /unknown or resolved interaction/);
  assert.match((await backend.close()).execution_error.message, /unknown or resolved interaction/);
});
async function initial(t, model, onItem) {
  const artifactDir = await mkdtemp(join(tmpdir(), 'due-resident-init-'));
  t.after(() => rm(artifactDir, { recursive: true, force: true }));
  const backend = await openRpc({ dueBinary, artifactDir, profile: { provider: 'codex', cwd: artifactDir, model }, onItem });
  t.after(async () => { try { await backend.close(); } catch {} });
  return backend;
}
test(`rpc: root Claude init supplies a null-open native ID before observers and survives failure`, async t => {
  const backend = await initial(t, 'init-hold', record => {
    if (record.data.kind === 'native_summary' && record.data.data.event.session_id === 'claude-native') {
      assert.equal(backend.getReceipt().native_session_id, 'claude-native'); // Captured before the observer ran.
      throw new Error('observer failed after init');
    }
  });
  assert.equal(backend.getReceipt().native_session_id, null);
  await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), /observer failed after init/);
  const receipt = await backend.close();
  assert.equal(receipt.native_session_id, 'claude-native');
  assert.equal(receipt.processes[0].exit_code, 0);
});
test(`rpc: the init ID survives a later protocol failure`, async t => {
  const backend = await initial(t, 'init-malformed');
  await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), /JSON/);
  const receipt = await backend.close();
  assert.equal(receipt.native_session_id, 'claude-native'); assert.equal(receipt.processes[0].exit_code, 0);
});
test(`rpc: a conflicting root init is rejected and the first ID is kept`, async t => {
  const backend = await initial(t, 'init-conflict');
  await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), /identity changed/);
  assert.equal((await backend.close()).native_session_id, 'claude-native');
});
test(`rpc: nested, non-init, non-Claude and assistant IDs are never adopted`, async t => {
  const backend = await initial(t, 'init-unrelated');
  await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), /JSON/);
  assert.equal((await backend.close()).native_session_id, null);
});
test(`rpc: abort cancels the active turn and waits for process cleanup on close`, async t => {
  const control = new AbortController();
  const { backend, artifactDir, items } = await setup(t, 'hold', control.signal);
  const turn = backend.turn({ id: 'held', prompt: 'wait' });
  while (!items.some(i => i.data.kind === 'turn_started')) await new Promise(resolve => setTimeout(resolve, 5));
  control.abort();
  await assert.rejects(turn, /cancelled/);
  assert.match((await backend.close()).execution_error.message, /cancelled/);
  assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
});

for (const [mode, pattern] of [['unknown-status', /Invalid Due turn result/], ['false-success', /Invalid Due turn result/], ['missing-id', /no native session ID/], ['wrong-run', /result identity changed/]]) {
  test(`rpc: malformed normalized result (${mode}) preserves evidence and fails`, async t => {
    const { backend, artifactDir } = await setup(t, mode);
    await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), pattern);
    assert.match((await backend.close()).execution_error.message, pattern);
    assert.equal(JSON.parse(await readFile(join(artifactDir, 'first/result.json'))).text, 'turn 1');
    assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
  });
}
test(`rpc: a rejected async item callback is handled and fails explicitly`, async t => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'due-resident-async-'));
  t.after(() => rm(artifactDir, { recursive: true, force: true }));
  await assert.rejects(openRpc({ dueBinary, artifactDir, profile: { provider: 'codex', cwd: artifactDir, model: 'early' },
    onItem: () => Promise.reject(new Error('async callback failure')) }), /onItem must be synchronous/);
  assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
});

test('rpc: direct error responses are surfaced and cleanup still runs', async t => {
  const { backend, artifactDir } = await setup(t, 'rpc-error');
  await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), /-32000.*Fixture request rejected/);
  assert.match((await backend.close()).execution_error.message, /Fixture request rejected/);
  assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
});
test('rpc: unsupported protocol version fails before starting turns', async t => {
  await assert.rejects(setup(t, 'wrong-version'), /Unsupported Due RPC session protocol/);
});
