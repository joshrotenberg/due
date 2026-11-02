#!/usr/bin/env node
// Two turns in one resident session; stdout is the complete observed item stream.
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { withSession } from './session.mjs';
import { correctionWorkflow } from './correction.mjs';

const { values } = parseArgs({ options: {
  due: { type: 'string', default: 'due' },
  provider: { type: 'string', default: 'codex' },
  cwd: { type: 'string', default: '.' },
  artifacts: { type: 'string' },
  model: { type: 'string' },
} });
if (!values.artifacts) throw new Error('usage: node rpc-example.mjs --artifacts FRESH_DIR [--due PATH] [--provider claude|codex] [--cwd DIR] [--model ID]');
const controller = new AbortController();
const handlers = new Map(['SIGINT', 'SIGTERM', 'SIGHUP'].map(signal => {
  const handler = () => controller.abort(new Error(`caller received ${signal}`));
  process.on(signal, handler);
  return [signal, handler];
}));
try {
  const profile = { provider: values.provider, cwd: resolve(values.cwd), timeout: '2m', effort: 'low' };
  if (values.model) profile.model = values.model;
  const output = await withSession({
    backend: 'rpc', dueBinary: values.due, profile,
    artifactDir: resolve(values.artifacts), maxTurns: 2, signal: controller.signal,
    onItem: record => { process.stdout.write(`${JSON.stringify(record)}\n`); },
  }, correctionWorkflow);
  process.stderr.write(`Validated both turns: ${JSON.stringify(output)}\n`);
} finally {
  for (const [signal, handler] of handlers) process.off(signal, handler);
}
