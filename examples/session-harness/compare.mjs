#!/usr/bin/env node
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { parseArgs } from 'node:util';
import { withSession } from './session.mjs';
import { correctionWorkflow } from './correction.mjs';

const { values } = parseArgs({ options: {
  provider: { type: 'string', default: 'codex' },
  backend: { type: 'string', default: 'both' },
  due: { type: 'string', default: 'due' },
  cwd: { type: 'string', default: '.' },
  artifacts: { type: 'string' },
  model: { type: 'string' },
  effort: { type: 'string', default: 'low' },
} });
if (!values.artifacts || !['both', 'all', 'resume', 'resident', 'rpc'].includes(values.backend)) {
  throw new Error('usage: node compare.mjs --artifacts FRESH_DIR [--due PATH] [--provider claude|codex] [--backend both|all|resume|resident|rpc] [--cwd DIR]');
}
const artifacts = resolve(values.artifacts);
await mkdir(artifacts, { mode: 0o700 });
const controller = new AbortController();
const handlers = new Map(['SIGINT', 'SIGTERM', 'SIGHUP'].map(signal => {
  const handler = () => controller.abort(new Error(`caller received ${signal}`));
  process.on(signal, handler);
  return [signal, handler];
}));
const profile = { provider: values.provider, cwd: resolve(values.cwd), effort: values.effort, timeout: '2m' };
if (values.model) profile.model = values.model;
const comparison = { profile, workflow: 'two-turn-correction', arms: [] };
try {
  const backends = values.backend === 'all' ? ['resume', 'resident', 'rpc']
    : values.backend === 'both' ? ['resume', 'resident'] : [values.backend];
  for (const backend of backends) {
    const artifactDir = join(artifacts, backend);
    const started = performance.now();
    process.stderr.write(`${backend}: starting two-turn correction\n`);
    const output = await withSession({ backend, dueBinary: values.due, profile, artifactDir,
      maxTurns: 2, signal: controller.signal }, correctionWorkflow);
    const receipt = JSON.parse(await readFile(join(artifactDir, 'session.json'), 'utf8'));
    const arm = { backend, duration_ms: Math.round(performance.now() - started), output,
      native_session_id: receipt.native_session_id,
      open_duration_ms: receipt.open_duration_ms, close_duration_ms: receipt.close_duration_ms,
      turns: receipt.turns.map(({ id, duration_ms, process, status }) => ({ id, duration_ms, process, status })),
      processes: receipt.processes, events: receipt.events, capabilities: receipt.capabilities };
    comparison.arms.push(arm);
    process.stderr.write(`${backend}: both answers validated; ${arm.processes.length} Due process(es), ${arm.events.count} observed records\n`);
  }
  console.log(JSON.stringify(comparison, null, 2));
} catch (error) {
  comparison.error = String(error.message ?? error);
  process.stderr.write(`comparison stopped: ${comparison.error}\n`);
  process.exitCode = 1;
} finally {
  await writeFile(join(artifacts, 'comparison.json'), `${JSON.stringify(comparison, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  for (const [signal, handler] of handlers) process.off(signal, handler);
}
