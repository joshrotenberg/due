import assert from 'node:assert/strict';
import { chmod, copyFile, mkdtemp, mkdir, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { openResume } from './resume.mjs';

async function setup(t, overrides = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'due-resume-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const dueBinary = join(dir, 'due');
  await copyFile(fileURLToPath(new URL('./fake-resume-due.mjs', import.meta.url)), dueBinary);
  await chmod(dueBinary, 0o700);
  const artifactDir = join(dir, 'artifacts');
  await mkdir(artifactDir, { mode: 0o700 });
  const items = [];
  const backend = await openResume({ dueBinary, artifactDir,
    profile: { provider: 'codex', cwd: dir, effort: 'low', timeout: '1m' },
    onItem: item => { items.push(item); }, ...overrides });
  t.after(() => backend.close());
  return { dir, artifactDir, items, backend };
}

async function waitFor(path) {
  const deadline = Date.now() + 3000;
  while (Date.now() < deadline) {
    try { await stat(path); return; } catch {}
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  throw new Error(`Fixture did not create ${path}`);
}

test('two turns use fresh processes and the same native resume ID, with raw receipts', async t => {
  const { backend, dir, artifactDir, items } = await setup(t);
  const first = await backend.turn({ id: 'first', prompt: 'remember' });
  const second = await backend.turn({ id: 'second', prompt: 'recall' });
  assert.equal(second.result.text, 'remembered');
  assert.equal(first.native_session_id, second.native_session_id);
  assert.notEqual(first.process.pid, second.process.pid);
  const requests = (await readFile(join(dir, 'requests.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
  assert.equal(requests[0].resume, undefined);
  assert.equal(requests[1].resume, 'fixture-native');
  assert.equal(requests[1].ephemeral, false);
  assert.equal(requests[1].timeout, '1m');
  assert.equal(items.length, 2);
  assert.equal(items[1].source, 'due_trace');
  assert.equal(items[1].turn_id, 'second');
  assert.match(await readFile(join(artifactDir, 'first/stderr.log'), 'utf8'), /fixture diagnostic/);
  assert.equal(JSON.parse(await readFile(join(artifactDir, 'second/exit.json'), 'utf8')).exit_code, 0);
  assert.equal((await backend.close()).processes.length, 2);
  await assert.rejects(backend.turn({ id: 'third', prompt: 'no' }), /closed/);
});

test('failed native turns retain process exit and remain deliberately resumable', async t => {
  const { backend } = await setup(t);
  const failed = await backend.turn({ id: 'failed', prompt: 'fail' });
  assert.equal(failed.result.status, 'failed');
  assert.equal(failed.process.exit_code, 1);
  const next = await backend.turn({ id: 'next', prompt: 'recall' });
  assert.equal(next.result.text, 'remembered');
});

test('missing identity, identity changes, malformed JSON and contradictory exits fail explicitly', async t => {
  for (const prompt of ['missing', 'mismatch', 'malformed', 'success-nonzero']) {
    await t.test(prompt, async t => {
      const { backend, artifactDir } = await setup(t);
      if (prompt === 'mismatch') await backend.turn({ id: 'first', prompt: 'remember' });
      if (prompt === 'missing') assert.equal((await backend.turn({ id: 'bad', prompt })).result.status, 'failed');
      else await assert.rejects(backend.turn({ id: 'bad', prompt }));
      await assert.rejects(backend.turn({ id: 'after', prompt: 'recall' }));
      assert.ok((await readFile(join(artifactDir, 'bad/result.json'), 'utf8')).length);
      assert.equal(JSON.parse(await readFile(join(artifactDir, 'bad/exit.json'), 'utf8')).signal, null);
    });
  }
});

test('close and parent abort both join an active Due process after SIGINT', async t => {
  for (const abortParent of [false, true]) {
    await t.test(abortParent ? 'parent abort' : 'close', async t => {
      const controller = new AbortController();
      const { backend, dir } = await setup(t, { signal: controller.signal });
      const running = backend.turn({ id: 'held', prompt: 'hold' });
      await waitFor(join(dir, 'ready'));
      await assert.rejects(backend.turn({ id: 'overlap', prompt: 'no' }), /already active/);
      if (abortParent) controller.abort();
      const closed = await backend.close();
      const ended = await running;
      assert.equal(ended.result.status, 'cancelled');
      assert.equal(ended.process.exit_code, 130);
      assert.equal(closed.processes[0].cancel_requested, true);
      assert.equal(await readFile(join(dir, 'cancelled'), 'utf8'), 'yes');
      assert.throws(() => process.kill(ended.process.pid, 0), { code: 'ESRCH' });
    });
  }
});

test('failed spawn preserves launch evidence and never fabricates a process exit', async t => {
  const { backend, artifactDir } = await setup(t, { dueBinary: '/no-such-due-binary' });
  await assert.rejects(backend.turn({ id: 'spawn', prompt: 'test' }), /ENOENT/);
  const exit = JSON.parse(await readFile(join(artifactDir, 'spawn/exit.json'), 'utf8'));
  assert.equal(exit.exit_code, null);
  assert.equal(exit.pid, null);
  assert.equal(exit.launch_error.code, 'ENOENT');
});

test('non-Error callback failures reject after preserving process evidence', async t => {
  for (const thrown of [null, 0, 'callback failed']) {
    await t.test(String(thrown), async t => {
      const { backend, artifactDir } = await setup(t, { onItem() { throw thrown; } });
      await assert.rejects(backend.turn({ id: 'callback', prompt: 'remember' }), error => error.message === String(thrown));
      const exit = JSON.parse(await readFile(join(artifactDir, 'callback/exit.json'), 'utf8'));
      assert.equal(exit.collection_error.message, String(thrown));
      assert.equal(exit.cancel_requested, true);
      assert.throws(() => process.kill(exit.pid, 0), { code: 'ESRCH' });
    });
  }
});
