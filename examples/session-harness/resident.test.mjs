import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { openResident, openRpc } from './resident.mjs';
const dueBinary = fileURLToPath(new URL('./resident.fixture.mjs', import.meta.url));
for (const [transport, open] of [['mcp', openResident], ['rpc', openRpc]]) {
async function setup(t, model = 'early', signal) {
  const artifactDir = await mkdtemp(join(tmpdir(), 'due-resident-'));
  t.after(() => rm(artifactDir, { recursive: true, force: true }));
  const items = [];
  const backend = await open({ dueBinary, artifactDir, profile: { provider: 'codex', cwd: artifactDir, model }, onItem: item => items.push(item), signal });
  t.after(async () => { try { await backend.close(); } catch {} });
  return { backend, items, artifactDir };
}
test(`${transport}: two turns share a process, early results and final output are retained`, async t => {
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
  const names = input.map(i => transport === 'rpc' ? i.method.replace('.', '_') : i.params?.name);
  assert.equal(names.filter(name => name === 'session_open').length, 1);
  assert.equal(names.filter(name => name === 'turn_read').length, 0);
  if (transport === 'rpc') {
    const wire = (await readFile(join(artifactDir, 'protocol.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
    assert.ok(wire.filter(row => row.direction === 'in').every(row => !['initialize', 'notifications/initialized', 'tools/call'].includes(row.message.method)));
    assert.ok(wire.some(row => row.direction === 'out' && row.message.method === 'session.item'));
  }
  assert.equal((JSON.parse(await readFile(join(artifactDir, 'first/result.json')))).run_id, 'native-turn-1');
});
test(`${transport}: cursor gaps recover full items once`, async t => {
  const { backend, items } = await setup(t, 'gap');
  await backend.turn({ id: 'first', prompt: 'one' }); await backend.close();
  assert.deepEqual(items.map(i => i.data.cursor), [1, 2, 3, 4, 5]);
  assert.equal(items.filter(i => i.data.kind === 'native').length, 1);
});
test(`${transport}: native failures are data and preserve the resident process`, async t => {
  const { backend } = await setup(t, 'failed');
  assert.equal((await backend.turn({ id: 'first', prompt: 'one' })).result.status, 'failed');
  assert.equal((await backend.turn({ id: 'second', prompt: 'two' })).result.status, 'failed');
  assert.equal((await backend.close()).processes[0].exit_code, 0);
});
for (const [mode, pattern] of [['lost', /journal lost/], ['malformed', /JSON/], ['interaction', /explicit decision/]]) {
  test(`${transport}: ${mode} errors stay explicit and the process is settled`, async t => {
    const { backend, artifactDir } = await setup(t, mode);
    await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), pattern);
    assert.match((await backend.close()).execution_error.message, pattern);
    assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
    assert.equal(JSON.parse(await readFile(join(artifactDir, 'resident-exit.json'))).exit_code, 0);
  });
}
test(`${transport}: abort cancels the active turn and waits for process cleanup on close`, async t => {
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
  test(`${transport}: malformed normalized result (${mode}) preserves evidence and fails`, async t => {
    const { backend, artifactDir } = await setup(t, mode);
    await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), pattern);
    assert.match((await backend.close()).execution_error.message, pattern);
    assert.equal(JSON.parse(await readFile(join(artifactDir, 'first/result.json'))).text, 'turn 1');
    assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
  });
}
test(`${transport}: a rejected async item callback is handled and fails explicitly`, async t => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'due-resident-async-'));
  t.after(() => rm(artifactDir, { recursive: true, force: true }));
  await assert.rejects(open({ dueBinary, artifactDir, profile: { provider: 'codex', cwd: artifactDir, model: 'early' },
    onItem: () => Promise.reject(new Error('async callback failure')) }), /onItem must be synchronous/);
  assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
});

if (transport === 'rpc') {
  test('rpc: direct error responses are surfaced and cleanup still runs', async t => {
    const { backend, artifactDir } = await setup(t, 'rpc-error');
    await assert.rejects(backend.turn({ id: 'first', prompt: 'one' }), /-32000.*Fixture request rejected/);
    assert.match((await backend.close()).execution_error.message, /Fixture request rejected/);
    assert.equal(await readFile(join(artifactDir, 'fixture-exited'), 'utf8'), 'closed\n');
  });
  test('rpc: unsupported protocol version fails before starting turns', async t => {
    await assert.rejects(setup(t, 'wrong-version'), /Unsupported Due RPC session protocol/);
  });
}
}
