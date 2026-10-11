import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const here = path.dirname(fileURLToPath(import.meta.url));
const launcher = path.join(here, 'run-due.mjs');
const validator = path.join(here, 'validate-review.mjs');
const fakeSource = path.join(here, 'fake-due.fixture.mjs');
const readJson = async (file) => JSON.parse(await fs.readFile(file, 'utf8'));

function start(script, args, options = {}) {
  const { nodeArgs = [], ...spawnOptions } = options;
  const child = spawn(process.execPath, [...nodeArgs, script, ...args], {
    ...spawnOptions, stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8').on('data', (chunk) => { stdout += chunk; });
  child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
  const done = new Promise((resolve, reject) => {
    child.on('error', reject);
    child.on('close', (code, signal) => resolve({ code, signal, stdout, stderr }));
  });
  return { child, done };
}

async function fixture(t, prompt = 'Review only.\nUnicode: café\nLiteral: $(echo nope)\n') {
  const root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'due-caller-test-')));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const dir = path.join(root, 'assignments with spaces');
  await fs.mkdir(dir);
  const binary = path.join(root, 'fake due');
  await fs.copyFile(fakeSource, binary);
  // Node determines ES modules by extension; use an executable .mjs path.
  await fs.rename(binary, `${binary}.mjs`);
  await fs.chmod(`${binary}.mjs`, 0o700);
  await fs.writeFile(path.join(dir, 'pr-101.request.json'), '{"schema_version":1,"provider":"codex"}\n');
  await fs.writeFile(path.join(dir, 'pr-101.prompt.txt'), prompt);
  const env = {
    ...process.env,
    PATH: `${path.dirname(process.execPath)}${path.delimiter}${process.env.PATH ?? ''}`,
    FAKE_DUE_CAPTURE: path.join(root, 'received.json'),
    FAKE_DUE_READY: path.join(root, 'ready'),
    FAKE_DUE_CLEANUP: path.join(root, 'cleanup'),
  };
  return { root, dir, binary: `${binary}.mjs`, prompt, env };
}

function launch(f, extra = [], mode = 'success') {
  return start(launcher, [f.dir, 'pr-101', '--due', f.binary, ...extra], {
    cwd: f.root, env: { ...f.env, FAKE_DUE_MODE: mode },
  });
}

async function waitFor(file) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    try { return await fs.readFile(file, 'utf8'); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error(`Timed out waiting for ${file}`);
}

test('captures exact stdin and argv, complete output and private artifacts', { timeout: 10000 }, async (t) => {
  const f = await fixture(t);
  const run = await launch(f).done;
  assert.equal(run.code, 0, run.stderr);
  const received = await readJson(f.env.FAKE_DUE_CAPTURE);
  const out = path.join(f.dir, 'pr-101.run');
  assert.equal(Buffer.from(received.input, 'base64').toString(), f.prompt);
  assert.equal(received.cwd, f.root);
  assert.deepEqual(received.args, ['run', '--request', path.join(f.dir, 'pr-101.request.json'), '--stdin', '--output', 'json', '--trace-file', path.join(out, 'trace.jsonl')]);
  assert.equal(await fs.readFile(path.join(out, 'result.json'), 'utf8'), '{"fixture":true,"planned":false}\n');
  assert.equal(await fs.readFile(path.join(out, 'stderr.log'), 'utf8'), 'fixture diagnostic\n');
  const exit = await readJson(path.join(out, 'exit.json'));
  assert.equal(exit.exit_code, 0);
  assert.equal(exit.signal, null);
  assert.equal(exit.launch_error, null);
  assert.equal(exit.stdin_error, null);
  assert.ok(Date.parse(exit.finished_at) >= Date.parse(exit.started_at));
  assert.equal((await fs.stat(out)).mode & 0o777, 0o700);
  for (const name of ['result.json', 'stderr.log', 'exit.json']) {
    assert.equal((await fs.stat(path.join(out, name))).mode & 0o777, 0o600);
  }
});

test('dry run and real run have separate fresh directories; rerun cannot truncate', async (t) => {
  const f = await fixture(t);
  assert.equal((await launch(f, ['--dry-run']).done).code, 0);
  const planDir = path.join(f.dir, 'pr-101.plan');
  assert.equal((await readJson(path.join(planDir, 'plan.json'))).planned, true);
  await assert.rejects(fs.stat(path.join(planDir, 'trace.jsonl')), { code: 'ENOENT' });
  assert.equal((await launch(f).done).code, 0);
  const out = path.join(f.dir, 'pr-101.run');
  const names = await fs.readdir(out);
  const before = await Promise.all(names.map((name) => fs.readFile(path.join(out, name))));
  const repeat = await launch(f).done;
  assert.notEqual(repeat.code, 0);
  for (let i = 0; i < names.length; i++) assert.deepEqual(await fs.readFile(path.join(out, names[i])), before[i]);
});

test('invalid identifiers/options and missing inputs do not launch or create run artifacts', async (t) => {
  const f = await fixture(t);
  for (const args of [[f.dir, '../escape'], [f.dir, 'pr-101\n'], [f.dir, ''], [f.dir, 'a'.repeat(81)], [f.dir, 'pr-101', '--unknown'], [f.dir, 'pr-101', '--due']]) {
    const result = await start(launcher, args).done;
    assert.equal(result.code, 2, result.stderr);
  }
  await fs.unlink(path.join(f.dir, 'pr-101.prompt.txt'));
  assert.notEqual((await launch(f).done).code, 0);
  await assert.rejects(fs.stat(path.join(f.dir, 'pr-101.run')), { code: 'ENOENT' });
  await assert.rejects(fs.stat(f.env.FAKE_DUE_CAPTURE), { code: 'ENOENT' });
});

test('concurrent reuse of one ID starts only one child', async (t) => {
  const f = await fixture(t);
  const results = await Promise.all([launch(f).done, launch(f).done]);
  assert.equal(results.filter((result) => result.code === 0).length, 1);
  assert.equal(results.filter((result) => result.code !== 0).length, 1);
  const record = await readJson(path.join(f.dir, 'pr-101.run/exit.json'));
  assert.equal(record.exit_code, 0);
  assert.deepEqual(await readJson(path.join(f.dir, 'pr-101.run/result.json')), { fixture: true, planned: false });
});

test('missing executable saves a complete launch failure record', async (t) => {
  const f = await fixture(t);
  f.binary = path.join(f.root, 'does-not-exist');
  assert.notEqual((await launch(f).done).code, 0);
  const record = await readJson(path.join(f.dir, 'pr-101.run/exit.json'));
  assert.equal(record.exit_code, null);
  assert.equal(record.signal, null);
  assert.equal(record.launch_error.code, 'ENOENT');
  assert.ok(record.finished_at);
});

test('preserves a nonzero child exit without replacing it with later processing', async (t) => {
  const f = await fixture(t);
  assert.equal((await launch(f, [], 'exit-seven').done).code, 7);
  assert.equal((await readJson(path.join(f.dir, 'pr-101.run/exit.json'))).exit_code, 7);
});

test('closed child stdin is recorded and cannot become a successful caller run', { timeout: 10000 }, async (t) => {
  const f = await fixture(t, Buffer.alloc(8 * 1024 * 1024, 120));
  const result = await launch(f, [], 'close-stdin').done;
  assert.notEqual(result.code, 0);
  const record = await readJson(path.join(f.dir, 'pr-101.run/exit.json'));
  assert.equal(record.exit_code, 0);
  assert.ok(record.stdin_error, 'stdin failure must be saved');
  assert.equal(record.launch_error, null);
});

test('forwards cancellation once and waits for Due cleanup/output', { timeout: 10000 }, async (t) => {
  const f = await fixture(t);
  const { child, done } = launch(f, [], 'signal');
  t.after(() => { if (child.exitCode === null) child.kill('SIGKILL'); });
  await waitFor(f.env.FAKE_DUE_READY);
  child.kill('SIGTERM');
  await waitFor(f.env.FAKE_DUE_CLEANUP);
  child.kill('SIGTERM');
  const result = await done;
  assert.equal(result.code, 130, result.stderr);
  assert.equal(await fs.readFile(f.env.FAKE_DUE_CLEANUP, 'utf8'), 'SIGTERM\n');
  const out = path.join(f.dir, 'pr-101.run');
  assert.equal(await fs.readFile(path.join(out, 'result.json'), 'utf8'), 'cleanup-finished\n');
  const record = await readJson(path.join(out, 'exit.json'));
  assert.equal(record.exit_code, 130);
  assert.equal(record.signal, null);
  assert.equal(record.signal_received, 'SIGTERM');
});

test('distinguishes native signal termination from a numeric exit', async (t) => {
  const f = await fixture(t);
  const result = await launch(f, [], 'self-signal').done;
  assert.equal(result.code, 143);
  const record = await readJson(path.join(f.dir, 'pr-101.run/exit.json'));
  assert.equal(record.exit_code, null);
  assert.equal(record.signal, 'SIGTERM');
});

test('a signal during exit-file writing cannot change the finalized child outcome', { timeout: 10000 }, async (t) => {
  const f = await fixture(t);
  const preload = path.join(f.root, 'signal-during-exit-write.mjs');
  // Test-only interposition delivers SIGTERM after serialization but before the
  // asynchronous exit-file write. No timing assumptions or production hooks.
  await fs.writeFile(preload, `
    import fs from 'node:fs/promises';
    import { syncBuiltinESMExports } from 'node:module';
    const originalOpen = fs.open;
    fs.open = async (...args) => {
      const handle = await originalOpen(...args);
      if (String(args[0]).endsWith('/exit.json')) {
        const originalWrite = handle.writeFile.bind(handle);
        handle.writeFile = async (...params) => {
          // Signal listeners alone do not keep Node's event loop alive.
          const keepAlive = setInterval(() => {}, 1000);
          try {
            const received = new Promise(resolve => process.once('SIGTERM', resolve));
            process.kill(process.pid, 'SIGTERM');
            await received;
            return await originalWrite(...params);
          } finally {
            clearInterval(keepAlive);
          }
        };
      }
      return handle;
    };
    syncBuiltinESMExports();
  `);
  const result = await start(launcher, [f.dir, 'pr-101', '--due', f.binary], {
    cwd: f.root, env: f.env, nodeArgs: ['--import', preload],
  }).done;
  assert.equal(result.code, 0, result.stderr);
  const record = await readJson(path.join(f.dir, 'pr-101.run/exit.json'));
  assert.equal(record.exit_code, 0);
  assert.equal(record.signal_received, null);
});

async function reviewFixture(t) {
  const f = await fixture(t);
  const run = path.join(f.dir, 'pr-101.run');
  await fs.mkdir(run);
  const envelope = {
    schema_version: 1, run_id: 'test-run', session_id: 'test-session', status: 'succeeded',
    structured_output_present: true, error: null, answer_schema_validation: 'not_performed',
    trace: { incomplete: false, dropped_records: 0 },
    structured_output: { pr: 101, verdict: 'request_changes', summary: 'One introduced defect.', findings: [
      { priority: 'P2', file: 'src/parser.rs', line: 12, description: 'Empty input panics.', validation: 'Read the new unchecked access.' },
    ], limitations: ['Static review; no tests run.'] },
  };
  const exit = { exit_code: 0, signal: null, launch_error: null, stdin_error: null, signal_received: null };
  const save = async () => {
    await fs.writeFile(path.join(run, 'result.json'), JSON.stringify(envelope));
    await fs.writeFile(path.join(run, 'exit.json'), JSON.stringify(exit));
  };
  await save();
  return { ...f, run, envelope, exit, save };
}

test('request_changes is valid completed work, with exclusive validation evidence', async (t) => {
  const f = await reviewFixture(t);
  const args = [f.dir, 'pr-101', '101'];
  const result = await start(validator, args).done;
  assert.equal(result.code, 0, result.stderr);
  const artifact = path.join(f.run, 'validated.json');
  const saved = await fs.readFile(artifact);
  assert.equal(JSON.parse(saved).valid, true);
  assert.equal(JSON.parse(saved).verdict, 'request_changes');
  assert.equal((await fs.stat(artifact)).mode & 0o777, 0o600);
  assert.notEqual((await start(validator, args).done).code, 0);
  assert.deepEqual(await fs.readFile(artifact), saved);
});

test('validator rejects transport failures and malformed task answers without acceptance artifacts', async (t) => {
  const cases = [
    ['nonzero exit', (f) => { f.exit.exit_code = 7; }],
    ['received cancellation', (f) => { f.exit.signal_received = 'SIGINT'; }],
    ['stdin error', (f) => { f.exit.stdin_error = { code: 'EPIPE' }; }],
    ['failed execution', (f) => { f.envelope.status = 'failed'; }],
    ['dropped trace', (f) => { f.envelope.trace.dropped_records = 1; }],
    ['wrong PR', (f) => { f.envelope.structured_output.pr = 102; }],
    ['zero line', (f) => { f.envelope.structured_output.findings[0].line = 0; }],
    ['unknown verdict', (f) => { f.envelope.structured_output.verdict = 'maybe'; }],
    ['extra answer key', (f) => { f.envelope.structured_output.approved = true; }],
    ['null answer', (f) => { f.envelope.structured_output = null; }],
  ];
  for (const [name, mutate] of cases) await t.test(name, async (st) => {
    const f = await reviewFixture(st);
    mutate(f); await f.save();
    const result = await start(validator, [f.dir, 'pr-101', '101']).done;
    assert.equal(result.code, 1, result.stderr);
    await assert.rejects(fs.stat(path.join(f.run, 'validated.json')), { code: 'ENOENT' });
  });
});
