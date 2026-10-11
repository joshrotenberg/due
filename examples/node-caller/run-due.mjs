#!/usr/bin/env node
// Optional caller example. Due owns provider execution and process cleanup.
import { spawn } from 'node:child_process';
import { constants as fsConstants } from 'node:fs';
import { access, mkdir, open, readFile, realpath, stat } from 'node:fs/promises';
import { constants as osConstants } from 'node:os';
import { join, resolve } from 'node:path';

function usageError(message) {
  return Object.assign(new Error(message), { exitCode: 2 });
}

function options(argv) {
  const [directory, id, ...flags] = argv;
  if (!directory || !id || id.length > 80 || !/^[A-Za-z0-9]/.test(id) || /[^A-Za-z0-9_-]/.test(id)) {
    throw usageError('usage: node run-due.mjs <assignment-dir> <id> [--dry-run] [--due PATH]');
  }
  let dryRun = false;
  let binary = 'due';
  let explicitBinary = false;
  for (let i = 0; i < flags.length; i += 1) {
    if (flags[i] === '--dry-run' && !dryRun) {
      dryRun = true;
    } else if (flags[i] === '--due' && !explicitBinary) {
      binary = flags[++i];
      if (!binary || binary.includes('\0') || binary.startsWith('--')) {
        throw usageError('--due needs an executable name or path');
      }
      explicitBinary = true;
    } else {
      throw usageError(`unknown or repeated option: ${flags[i]}`);
    }
  }
  return { directory, id, dryRun, binary };
}

function describe(error) {
  return error.code ? `${error.code}: ${error.message}` : error.message;
}

function errorRecord(error) {
  return { code: error.code ?? null, message: error.message };
}

async function readableFile(path) {
  if (!(await stat(path)).isFile()) throw new Error(`not a regular input file: ${path}`);
  await access(path, fsConstants.R_OK);
}

function signalExit(signal) {
  const number = osConstants.signals[signal];
  return number ? 128 + number : 1;
}

function callerExit(record) {
  if (record.exit_code !== null && record.exit_code !== 0) return record.exit_code;
  if (record.signal) return signalExit(record.signal);
  if (record.signal_received && record.exit_code === 0) return signalExit(record.signal_received);
  if (record.launch_error || record.stdin_error || record.exit_code === null) return 1;
  return 0;
}

async function main() {
  const { directory, id, dryRun, binary } = options(process.argv.slice(2));
  const dir = await realpath(resolve(directory));
  const request = join(dir, `${id}.request.json`);
  const promptPath = join(dir, `${id}.prompt.txt`);
  await Promise.all([readableFile(request), readableFile(promptPath)]);
  const prompt = await readFile(promptPath);
  const outputDir = join(dir, `${id}.${dryRun ? 'plan' : 'run'}`);
  // This reservation is exclusive: failed/partial attempts also keep their ID.
  await mkdir(outputDir, { mode: 0o700 });
  const args = ['run', '--request', request, '--stdin', '--output', 'json'];
  if (dryRun) args.push('--dry-run');
  else args.push('--trace-file', join(outputDir, 'trace.jsonl'));
  const record = {
    command: binary,
    args,
    started_at: new Date().toISOString(),
    finished_at: null,
    exit_code: null,
    signal: null,
    launch_error: null,
    stdin_error: null,
    signal_received: null,
  };
  const handles = [];
  let child;
  let spawned = false;
  let forwarded = false;
  let finalized = false;
  const handlers = new Map();
  function forwardSignal() {
    if (!spawned || forwarded || !record.signal_received) return;
    forwarded = true;
    // A child that already exited can make kill return false; close still settles it.
    try {
      child.kill(record.signal_received);
    } catch (error) {
      process.stderr.write(`could not forward ${record.signal_received}: ${describe(error)}\n`);
    }
  }
  async function exclusiveFile(name) {
    const handle = await open(join(outputDir, name), 'wx', 0o600);
    handles.push(handle);
    return handle;
  }
  try {
    const exitFile = await exclusiveFile('exit.json');
    for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
      const handler = () => {
        // Cancellation ends at child close. Later signals must not change an
        // outcome while its evidence is being written or descriptors closed.
        if (finalized || record.signal_received) return;
        record.signal_received = signal;
        forwardSignal(); // Queued until the spawn event if the child is not ready.
      };
      handlers.set(signal, handler);
      process.on(signal, handler);
    }
    try {
      const stdout = await exclusiveFile(dryRun ? 'plan.json' : 'result.json');
      const stderr = await exclusiveFile('stderr.log');
      await new Promise((done) => {
        try {
          child = spawn(binary, args, { shell: false, stdio: ['pipe', stdout.fd, stderr.fd] });
        } catch (error) {
          record.launch_error = errorRecord(error);
          done();
          return;
        }
        child.once('spawn', () => {
          spawned = true;
          forwardSignal();
        });
        child.on('error', (error) => { record.launch_error = errorRecord(error); });
        child.stdin.on('error', (error) => { record.stdin_error = errorRecord(error); });
        child.once('close', (code, signal) => {
          // Failed spawn can report a negative internal code; no child exited then.
          record.exit_code = spawned && Number.isInteger(code) ? code : null;
          record.signal = signal;
          finalized = true;
          done();
        });
        try {
          child.stdin.end(prompt, (error) => {
            if (error) record.stdin_error = errorRecord(error);
          });
        } catch (error) {
          record.stdin_error = errorRecord(error);
        }
      });
    } catch (error) {
      record.launch_error = errorRecord(error);
    }
    finalized = true;
    record.finished_at = new Date().toISOString();
    const snapshot = { ...record };
    await exitFile.writeFile(`${JSON.stringify(snapshot, null, 2)}\n`);
    return callerExit(snapshot);
  } finally {
    for (const handle of handles) await handle.close();
    for (const [signal, handler] of handlers) process.off(signal, handler);
  }
}

try {
  process.exitCode = await main();
} catch (error) {
  process.stderr.write(`run-due: ${describe(error)}\n`);
  process.exitCode = error.exitCode ?? 1;
}
