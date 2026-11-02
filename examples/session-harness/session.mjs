// A caller-owned, scoped conversation. Node 22+ built-ins only.
import { mkdir, writeFile } from 'node:fs/promises';
import { closeSync, openSync, writeSync } from 'node:fs';
import { join } from 'node:path';
import { openResume } from './resume.mjs';
import { normalizeTurnWaitMs, openRpc } from './resident.mjs';
import { sessionProfile } from './profile.mjs';

const describe = error => String(error?.message ?? error);
const asError = error => error instanceof Error ? error : new Error(describe(error));

/** All backends preserve conversation context; the RPC backend provides native items. */
export async function withSession(options, body) {
  const { backend: mode, dueBinary = 'due', artifactDir, onItem, onInteraction, signal, maxTurns = 4, turnWaitMs } = options;
  if (!['resume', 'rpc'].includes(mode)) throw new Error('backend must be resume or rpc');
  if (!artifactDir || typeof body !== 'function') throw new Error('artifactDir and callback are required');
  if (!Number.isInteger(maxTurns) || maxTurns < 1 || maxTurns > 64) throw new Error('maxTurns must be 1..64');
  if (onInteraction !== undefined && typeof onInteraction !== 'function') throw new Error('onInteraction must be a function');
  if (onInteraction && mode === 'resume') throw new Error('onInteraction is unsupported by the resume backend; use rpc');
  if (turnWaitMs !== undefined && mode === 'resume') throw new Error('turnWaitMs is unsupported by the resume backend; use rpc');
  const waitMs = mode === 'resume' ? undefined : normalizeTurnWaitMs(turnWaitMs);
  const profile = await sessionProfile(options.profile, mode);
  signal?.throwIfAborted();
  await mkdir(artifactDir, { mode: 0o700 }); // Exclusive, including failed earlier attempts.
  const eventFile = openSync(join(artifactDir, 'items.jsonl'), 'wx', 0o600);
  const controller = new AbortController();
  const stop = () => controller.abort(signal.reason);
  signal?.addEventListener('abort', stop, { once: true });
  if (signal?.aborted) stop();
  const receipt = { backend: mode, profile, started_at: new Date().toISOString(), turns: [],
    events: { count: 0, bytes: 0 }, processes: [], controls: [], body_error: null, cleanup_error: null };
  if (mode !== 'resume') receipt.turn_wait_ms = waitMs;
  let backend, pending, pendingId, closed = false, value, failure, nativeId;
  const used = new Set(), controls = new Set(), interruptedTurns = new Set();
  function retain(snapshot, cleanupError) {
    if (!snapshot) return;
    receipt.processes = snapshot.processes;
    if (mode === 'rpc') receipt.resolved_options = snapshot.resolved_options ?? null;
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
    backend = await ({ resume: openResume, rpc: openRpc }[mode])({
      dueBinary, profile, artifactDir, onItem: item, onInteraction, signal: controller.signal, turnWaitMs: waitMs,
      onFailure: error => controller.abort(error),
    });
    receipt.open_duration_ms = Math.round(performance.now() - opening);
    receipt.capabilities = backend.capabilities;
    if (mode === 'rpc') receipt.resolved_options = backend.getReceipt().resolved_options;
    const agent = Object.freeze({
      capabilities: Object.freeze({ ...backend.capabilities }),
      signal: controller.signal,
      interrupt({ id } = {}) {
        if (closed) return Promise.reject(new Error('session scope is closed'));
        if (!backend.capabilities.interrupt) return Promise.reject(new Error('interrupt is unsupported by this backend; use rpc'));
        if (!pending || pendingId !== id) return Promise.reject(new Error('no active turn with that caller ID'));
        if (controller.signal.aborted) return Promise.reject(controller.signal.reason);
        if (controls.size) return Promise.reject(new Error('an interruption is already pending'));
        if (interruptedTurns.has(id)) return Promise.reject(new Error('interruption already requested for this turn'));
        interruptedTurns.add(id);
        const started = performance.now();
        const record = { method: 'turn.interrupt', id, started_at: new Date().toISOString() };
        receipt.controls.push(record);
        const operation = backend.interrupt({ id }).then(result => {
          record.result = result; return result;
        }, error => { record.error = describe(error); throw error; }).finally(() => {
          record.duration_ms = Math.round(performance.now() - started); controls.delete(operation);
        });
        controls.add(operation); operation.catch(() => {});
        return operation;
      },
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
        pendingId = id;
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
            pending = null; pendingId = undefined;
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
    if (controls.size) throw new Error('session scope returned with an active control; await all work');
  } catch (error) {
    failure = asError(error);
    receipt.body_error = describe(error);
    retain(error?.sessionReceipt);
    if (error?.cleanup) receipt.cleanup_error = error.cleanup;
  } finally {
    closed = true;
    if (failure || pending || controls.size) controller.abort(failure ?? new Error('scope closed'));
    if (pending) await pending.catch(() => {});
    const closing = performance.now();
    try {
      if (backend) retain(await backend.close());
    } catch (error) {
      retain(backend?.getReceipt?.(), error);
      receipt.cleanup_error = describe(error);
      failure = failure ? new AggregateError([failure, asError(error)], 'session body and cleanup failed') : asError(error);
    }
    await Promise.allSettled([...controls]);
    controller.abort(failure ?? new Error('session scope is closed'));
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
