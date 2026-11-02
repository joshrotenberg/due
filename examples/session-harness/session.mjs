// A caller-owned, scoped conversation. Node 22+ built-ins only.
import { mkdir, realpath, stat, writeFile } from 'node:fs/promises';
import { closeSync, openSync, writeSync } from 'node:fs';
import { join } from 'node:path';
import { openResume } from './resume.mjs';
import { openResident, openRpc } from './resident.mjs';

const allowed = new Set(['provider', 'cwd', 'model', 'effort', 'timeout']);
const describe = error => String(error?.message ?? error);
const asError = error => error instanceof Error ? error : new Error(describe(error));

async function profileFor(input) {
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('profile must be an object');
  for (const key of Object.keys(input)) {
    if (!allowed.has(key)) throw new Error(`unsupported shared profile field: ${key}`);
  }
  if (!['claude', 'codex'].includes(input.provider)) throw new Error('provider must be claude or codex');
  for (const key of ['cwd', 'model', 'effort', 'timeout']) {
    if (input[key] !== undefined && (typeof input[key] !== 'string' || !input[key].trim())) {
      throw new Error(`${key} must be a nonempty string`);
    }
  }
  if (input.effort !== undefined && !['low', 'medium', 'high'].includes(input.effort)) {
    throw new Error('effort must be low, medium or high');
  }
  const cwd = await realpath(input.cwd ?? '.');
  if (!(await stat(cwd)).isDirectory()) throw new Error('cwd must be a directory');
  return Object.freeze({ ...input, cwd });
}

/** All backends preserve conversation context; persistent backends provide native items. */
export async function withSession(options, body) {
  const { backend: mode, dueBinary = 'due', artifactDir, onItem, signal, maxTurns = 4 } = options;
  if (!['resume', 'resident', 'rpc'].includes(mode)) throw new Error('backend must be resume, resident or rpc');
  if (!artifactDir || typeof body !== 'function') throw new Error('artifactDir and callback are required');
  if (!Number.isInteger(maxTurns) || maxTurns < 1 || maxTurns > 64) throw new Error('maxTurns must be 1..64');
  const profile = await profileFor(options.profile);
  signal?.throwIfAborted();
  await mkdir(artifactDir, { mode: 0o700 }); // Exclusive, including failed earlier attempts.
  const eventFile = openSync(join(artifactDir, 'items.jsonl'), 'wx', 0o600);
  const controller = new AbortController();
  const stop = () => controller.abort(signal.reason);
  signal?.addEventListener('abort', stop, { once: true });
  if (signal?.aborted) stop();
  const receipt = { backend: mode, profile, started_at: new Date().toISOString(), turns: [],
    events: { count: 0, bytes: 0 }, processes: [], body_error: null, cleanup_error: null };
  let backend, pending, closed = false, value, failure, nativeId;
  const used = new Set();
  function retain(snapshot, cleanupError) {
    if (!snapshot) return;
    receipt.processes = snapshot.processes;
    nativeId ??= snapshot.native_session_id;
    if (!failure && snapshot.execution_error && snapshot.execution_error !== cleanupError) {
      failure = asError(snapshot.execution_error);
      receipt.body_error = describe(failure);
    }
  }
  function item(record) {
    const line = Buffer.from(`${JSON.stringify(record)}\n`);
    if (receipt.events.bytes + line.length > 64 * 1024 * 1024) throw new Error('session transcript exceeds 64 MiB');
    let offset = 0;
    while (offset < line.length) offset += writeSync(eventFile, line, offset);
    receipt.events.bytes += line.length;
    receipt.events.count += 1;
    let returned;
    try { returned = onItem?.(record); }
    catch (error) { throw asError(error); }
    if (returned && typeof returned.then === 'function') {
      // Prevent an unhandled callback rejection; asynchronous observation is not supported.
      Promise.resolve(returned).catch(() => {});
      throw new Error('onItem must be synchronous; persist or enqueue without awaiting');
    }
  }
  try {
    const opening = performance.now();
    backend = await ({ resume: openResume, resident: openResident, rpc: openRpc }[mode])({
      dueBinary, profile, artifactDir, onItem: item, signal: controller.signal,
    });
    receipt.open_duration_ms = Math.round(performance.now() - opening);
    receipt.capabilities = backend.capabilities;
    const agent = Object.freeze({
      capabilities: Object.freeze({ ...backend.capabilities }),
      turn({ id, prompt }) {
        if (closed) return Promise.reject(new Error('session scope is closed'));
        if (pending) return Promise.reject(new Error('one turn at a time; await the current turn'));
        if (typeof id !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9_-]{0,79}$/.test(id)) {
          return Promise.reject(new Error('turn id must be 1..80 safe filename characters'));
        }
        if (used.has(id) || used.size >= maxTurns) return Promise.reject(new Error('duplicate turn id or maxTurns exceeded'));
        if (typeof prompt !== 'string' || !prompt.trim()) return Promise.reject(new Error('prompt must be nonempty'));
        if (controller.signal.aborted) return Promise.reject(new Error('session was cancelled'));
        used.add(id);
        const started = performance.now();
        const turnReceipt = { id, started_at: new Date().toISOString() };
        receipt.turns.push(turnReceipt);
        pending = (async () => {
          try {
            const turn = await backend.turn({ id, prompt });
            const idNow = turn.native_session_id ?? turn.result?.session_id;
            if (nativeId && idNow && nativeId !== idNow) throw new Error('native session identity changed');
            nativeId ??= idNow;
            Object.assign(turnReceipt, { native_session_id: idNow ?? null,
              status: turn.result.status, native_run_id: turn.result.run_id ?? null,
              process: turn.process ?? null });
            return turn.result;
          } catch (error) {
            turnReceipt.error = describe(error);
            throw error;
          } finally {
            turnReceipt.duration_ms = Math.round(performance.now() - started);
            pending = null;
          }
        })();
        // Scope cleanup observes abandoned rejections even when user code forgets await.
        pending.catch(() => {});
        return pending;
      },
    });
    value = await body(agent);
    controller.signal.throwIfAborted();
    if (pending) throw new Error('session scope returned with an active turn; await all work');
  } catch (error) {
    failure = asError(error);
    receipt.body_error = describe(error);
    retain(error?.sessionReceipt);
    if (error?.cleanup) receipt.cleanup_error = error.cleanup;
  } finally {
    closed = true;
    if (failure || pending) controller.abort(failure ?? new Error('scope closed'));
    if (pending) await pending.catch(() => {});
    const closing = performance.now();
    try {
      if (backend) retain(await backend.close());
    } catch (error) {
      retain(backend?.getReceipt?.(), error);
      receipt.cleanup_error = describe(error);
      failure = failure ? new AggregateError([failure, asError(error)], 'session body and cleanup failed') : asError(error);
    }
    receipt.close_duration_ms = Math.round(performance.now() - closing);
    signal?.removeEventListener('abort', stop);
    closeSync(eventFile);
    receipt.native_session_id = nativeId ?? null;
    receipt.finished_at = new Date().toISOString();
    await writeFile(join(artifactDir, 'session.json'), `${JSON.stringify(receipt, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  }
  if (failure) throw failure;
  return value;
}
