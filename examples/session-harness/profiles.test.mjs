import assert from 'node:assert/strict';
import { mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { sessionProfile } from './profile.mjs';

test('RPC file profiles preserve omitted values and explicit null/false/empty overrides', async () => {
  const input = { request: 'profiles/worker.json', timeout: null, resume: null, ephemeral: false, native_args: [] };
  const profile = await sessionProfile(input, 'rpc');
  assert.deepEqual(profile, { ...input, request: resolve(input.request) });
  assert.equal(Object.hasOwn(profile, 'provider'), false);
  assert.equal(Object.hasOwn(profile, 'cwd'), false);
  assert.equal(Object.isFrozen(profile), true);
  input.native_args.push('--caller-mutated');
  assert.deepEqual(profile.native_args, []);
  assert.equal(Object.isFrozen(profile.native_args), true);
});

test('RPC direct options preserve nullable defaults and caller policy without interpreting native flags', async () => {
  const native_args = ['-c', 'approval_policy="never"'];
  const profile = await sessionProfile({ provider: 'codex', cwd: null, model: null, effort: null,
    timeout: null, binary: null, resume: null, ephemeral: true, native_args }, 'rpc');
  assert.equal(profile.cwd, null);
  assert.equal(profile.ephemeral, true);
  assert.deepEqual(profile.native_args, native_args);
  assert.notEqual(profile.native_args, native_args);
});

test('undefined overrides are omissions before profile-dependent validation', async () => {
  const profile = await sessionProfile({ request: 'worker.json', provider: undefined, cwd: undefined,
    ephemeral: undefined, native_args: undefined, timeout: undefined }, 'rpc');
  assert.deepEqual(profile, { request: resolve('worker.json') });
  const direct = await sessionProfile({ provider: 'codex', request: undefined }, 'rpc');
  assert.equal(Object.hasOwn(direct, 'request'), false);
  assert.equal(direct.cwd, await realpath('.'));
});

test('shared profiles retain canonical working directories and reject unsupported resume fields', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'due-profile-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const file = join(dir, 'file');
  await writeFile(file, 'not a directory');
  for (const backend of ['resume', 'rpc']) {
    assert.equal((await sessionProfile({ provider: 'claude', cwd: dir }, backend)).cwd, await realpath(dir));
    await assert.rejects(sessionProfile({ provider: 'claude', cwd: file }, backend), /cwd must be a directory/);
  }
  for (const extra of [{ request: file }, { native_args: [] }, { ephemeral: false }, { timeout: null }]) {
    await assert.rejects(sessionProfile({ provider: 'codex', ...extra }, 'resume'));
  }
});

test('RPC profile shape errors are rejected without reading files or spawning providers', async () => {
  for (const input of [null, [], {}, { request: '-' }, { request: null }, { request: '' },
    { request: 'worker.json', provider: null }, { provider: 'codex', ephemeral: null },
    { provider: 'codex', native_args: null }, { provider: 'codex', native_args: [1] },
    { provider: 'claude', model: 2 }, { provider: 'claude', effort: 'max' },
    { schema_version: 1, provider: 'codex' }, { provider: 'claude', prompt: 'hidden task' }]) {
    await assert.rejects(sessionProfile(input, 'rpc'));
  }
});
