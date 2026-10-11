#!/usr/bin/env node
// Offline test fixture: never invokes Due or a provider.
import fs from 'node:fs';

const args = process.argv.slice(2);
const mode = process.env.FAKE_DUE_MODE ?? 'success';
const capture = process.env.FAKE_DUE_CAPTURE;
const ready = process.env.FAKE_DUE_READY;
const cleanup = process.env.FAKE_DUE_CLEANUP;

if (mode === 'close-stdin') {
  fs.closeSync(0);
  setTimeout(() => process.exit(0), 100);
} else {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const input = Buffer.concat(chunks);
  if (capture) fs.writeFileSync(capture, JSON.stringify({ args, cwd: process.cwd(), input: input.toString('base64') }));

  if (mode === 'signal') {
    let stopping = false;
    for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
      process.on(signal, () => {
        fs.appendFileSync(cleanup, `${signal}\n`);
        if (stopping) return;
        stopping = true;
        setTimeout(() => {
          process.stdout.write('cleanup-finished\n');
          process.exit(130);
        }, 200);
      });
    }
    fs.writeFileSync(ready, 'ready');
    setInterval(() => {}, 1000);
  } else if (mode === 'self-signal') {
    process.kill(process.pid, 'SIGTERM');
  } else {
    const traceIndex = args.indexOf('--trace-file');
    if (traceIndex >= 0) fs.writeFileSync(args[traceIndex + 1], '{"fixture":true}\n', { flag: 'wx' });
    process.stderr.write('fixture diagnostic\n');
    process.stdout.write(JSON.stringify({ fixture: true, planned: args.includes('--dry-run') }));
    // A final delayed write makes premature file consumption observable.
    setTimeout(() => {
      process.stdout.write('\n');
      process.exit(mode === 'exit-seven' ? 7 : 0);
    }, 25);
  }
}
