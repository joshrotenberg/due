#!/usr/bin/env node
// Synthetic Due peer. No native provider or model is started.
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const argv = process.argv.slice(2);
if (JSON.stringify(argv) !== JSON.stringify(['run', '--request', '-', '--output', 'json', '--trace=json'])) process.exit(7);
appendFileSync(join(request.cwd, 'requests.jsonl'), `${JSON.stringify({ ...request, pid: process.pid })}\n`);
const native = request.prompt === 'mismatch' ? 'other-native' : 'fixture-native';
const trace = { schema_version: 1, run_id: `due-${process.pid}`, event: 'session_started', session_id: native, provider: request.provider };
process.stderr.write('fixture diagnostic\n' + JSON.stringify(trace) + '\n');
function finish(status = 'succeeded', code = 0, session = native) {
  const result = { schema_version: 1, provider: request.provider, status, session_id: session,
    text: request.resume ? 'remembered' : 'first', error: status === 'succeeded' ? null : {kind:status,message:'fixture failure'} };
  process.stdout.write(JSON.stringify(result));
  process.exitCode = code;
}
if (request.prompt === 'hold') {
  const interval = setInterval(() => {}, 1000);
  process.once('SIGINT', () => {
    clearInterval(interval);
    writeFileSync(join(request.cwd, 'cancelled'), 'yes');
    finish('cancelled', 130);
  });
  writeFileSync(join(request.cwd, 'ready'), String(process.pid));
} else if (request.prompt === 'malformed') {
  process.stdout.write('{bad');
} else if (request.prompt === 'missing') {
  finish('failed', 1, null);
} else if (request.prompt === 'fail') {
  finish('failed', 1);
} else if (request.prompt === 'success-nonzero') {
  finish('succeeded', 9);
} else {
  finish();
}
