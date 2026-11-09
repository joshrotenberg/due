// Opt-in integration with the real Due RPC binary and fake native providers only.
import assert from 'node:assert/strict';
import { chmod, copyFile, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { withSession } from './session.mjs';

test('Node supervises both native adapters using existing profiles and interrupt/reuse', {
  skip: !process.env.DUE_RPC_BINARY && 'set DUE_RPC_BINARY to the real RPC-enabled Due binary; no models are used',
  timeout: 30000,
}, async t => {
  const previous = process.env.DUE_SESSION_FIXTURE_DIR;
  t.after(() => {
    if (previous === undefined) delete process.env.DUE_SESSION_FIXTURE_DIR;
    else process.env.DUE_SESSION_FIXTURE_DIR = previous;
  });
  for (const provider of ['claude', 'codex']) {
    const dir = await mkdtemp(join(tmpdir(), 'due-node-native-'));
    t.after(() => rm(dir, { recursive: true, force: true }));
    const binary = join(dir, 'provider');
    await copyFile(fileURLToPath(new URL('../../crates/due-core/tests/fixtures/session-agent.sh', import.meta.url)), binary);
    await chmod(binary, 0o700);
    const profile = provider === 'codex'
      ? JSON.parse(await readFile(new URL('../codex-workspace-profile.json', import.meta.url), 'utf8'))
      : { schema_version: 1, provider, ephemeral: true,
        native_args: ['--allowedTools', 'Read', '--disallowedTools', 'Bash'] };
    profile.model = 'fixture-profile';
    const request = join(dir, 'profile.json');
    await writeFile(request, JSON.stringify(profile));
    process.env.DUE_SESSION_FIXTURE_DIR = dir;
    const artifactDir = join(dir, 'evidence');
    let decisions = 0;
    await withSession({
      backend: 'rpc', dueBinary: process.env.DUE_RPC_BINARY,
      profile: { request, cwd: dir, binary, timeout: '5s' }, artifactDir, maxTurns: 4,
      onInteraction() {
        decisions++;
        return provider === 'claude' ? { behavior: 'deny', message: 'fixture only' } : { decision: 'decline' };
      },
    }, async agent => {
      assert.equal(agent.capabilities.interrupt, true);
      assert.equal((await agent.turn({ id: 'first', prompt: 'first' })).status, 'succeeded');
      assert.equal((await agent.turn({ id: 'permission', prompt: 'ask' })).text, 'permission denied');
      assert.equal(decisions, 1);
      const running = agent.turn({ id: 'held', prompt: 'hold' });
      await agent.interrupt({ id: 'held' });
      await running;
      assert.equal(agent.signal.aborted, false);
      assert.equal((await agent.turn({ id: 'followup', prompt: 'next' })).status, 'succeeded');
    });
    const receipt = JSON.parse(await readFile(join(artifactDir, 'session.json'), 'utf8'));
    assert.equal(receipt.resolved_options.provider, provider);
    assert.equal(receipt.resolved_options.model, 'fixture-profile');
    assert.equal(receipt.resolved_options.ephemeral, profile.ephemeral);
    assert.deepEqual(receipt.resolved_options.native_args, profile.native_args);
    assert.equal(receipt.processes.length, 1);
    assert.equal(receipt.processes[0].exit_code, 0);
    assert.equal(receipt.processes[0].forced_kill, false);
    assert.equal(receipt.body_error, null);
    assert.equal(receipt.cleanup_error, null);
    assert.equal(new Set(receipt.turns.map(turn => turn.native_session_id)).size, 1);
    await readFile(join(dir, 'exited'));
  }
});
