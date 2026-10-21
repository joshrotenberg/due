#!/usr/bin/env node
// Offline transport fixture only. Never loads Due or a native provider.
import fs from 'node:fs';
import path from 'node:path';

const args = process.argv.slice(2);
const requestPath = args[args.indexOf('--request') + 1];
const requestBytes = fs.readFileSync(requestPath);
const request = JSON.parse(requestBytes);
const schema = fs.readFileSync(request.schema);
const root = path.dirname(requestPath);
const planned = args.includes('--dry-run');
const phase = planned ? 'plan' : 'run';
const mode = process.env.FAKE_SMOKE_MODE || 'success';
const input = [];
for await (const chunk of process.stdin) input.push(chunk);
const prompt = Buffer.concat(input);
function save(name, value) {
  fs.writeFileSync(path.join(root, name), JSON.stringify(value), { flag: 'wx', mode: 0o600 });
}
save(`${phase}.transport.json`, {
  args, request: requestBytes.toString('base64'), schema: schema.toString('base64'),
  prompt: prompt.toString('base64'), tmpdir: process.env.TMPDIR,
});
const result = {
  schema_version: 1, provider: request.provider, status: planned ? 'planned' : 'succeeded',
  text: '', structured_output: planned ? null : { summary: 'schema smoke' },
  structured_output_present: !planned, answer_schema_validation: 'not_performed',
  session_id: null, usage: null, cost_usd: null, duration_ms: 1, error: null,
};
if (planned) result.invocation = {
  binary: 'offline-fixture-only', cwd: request.cwd, args: [], stdin_bytes: prompt.length,
};
else {
  const tracePath = args[args.indexOf('--trace-file') + 1];
  fs.writeFileSync(tracePath, '{"fixture":true}\n', { flag: 'wx', mode: 0o600 });
}
let code = 0;
if ((!planned && ['reject', 'failed-valid'].includes(mode)) || (planned && mode === 'plan-error')) {
  result.status = 'failed';
  result.error = { kind: 'configuration', message: 'Synthetic rejection.', provider_exit_code: 7 };
  delete result.invocation;
  if (mode !== 'failed-valid') {
    result.structured_output = null;
    result.structured_output_present = false;
  }
  code = planned ? 2 : 7;
}
if (!planned && mode === 'wrong-answer') result.structured_output = { summary: 42 };
if (!planned && mode === 'keyword-ignored') result.structured_output = { summary: '' };
if (planned && mode === 'plan-wrong-shape') result.invocation = {};
if (planned && mode === 'remove-after-plan') fs.unlinkSync(process.argv[1]);
if (!planned && mode === 'self-signal') process.kill(process.pid, 'SIGTERM');
const waitsForSignal = (!planned && mode === 'signal') || (planned && mode === 'plan-signal');
if (waitsForSignal) {
  await new Promise((resolve) => {
    const keepAlive = setInterval(() => {}, 1000);
    for (const sig of ['SIGTERM', 'SIGINT', 'SIGHUP']) process.once(sig, () => {
      save(`${phase}.cleanup-started.json`, { signal: sig });
      setTimeout(() => {
        if (!planned) {
          result.status = 'cancelled';
          result.error = { kind: 'cancelled', message: 'Synthetic cancellation.', provider_exit_code: null };
          code = 130;
        }
        save(`${phase}.cleanup-finished.json`, true);
        clearInterval(keepAlive);
        resolve();
      }, 300);
    });
    save(`${phase}.ready.json`, true);
  });
}
process.stderr.write('offline fixture diagnostic\n');
if ((planned && mode === 'plan-malformed') || (!planned && mode === 'malformed')) process.stdout.write('{');
else if (planned && mode === 'plan-too-deep') {
  // Emit valid JSON beyond Python's nesting limit without recursively serializing it.
  process.stdout.write(`${JSON.stringify(result).slice(0, -1)},"deep":${'['.repeat(10000)}null${']'.repeat(10000)}}\n`);
}
else process.stdout.write(`${JSON.stringify(result)}\n`);
process.exitCode = code;
