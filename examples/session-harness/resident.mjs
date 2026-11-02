// Optional Node 22+ resident RPC client with lifecycle ownership and cursor recovery.
import { spawn } from 'node:child_process';
import { closeSync, mkdirSync, openSync, writeFileSync, writeSync } from 'node:fs';
import { join } from 'node:path';

const jsonFile = (path, value) => writeFileSync(path, JSON.stringify(value, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
function writeAll(fd, value) {
  const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
  let offset = 0;
  while (offset < bytes.length) {
    const written = writeSync(fd, bytes, offset, bytes.length - offset);
    if (written <= 0) throw new Error('Evidence write made no progress');
    offset += written;
  }
}
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); promise.catch(() => {}); return { promise, resolve, reject }; };
async function bounded(promise, ms, message) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(message)), ms); })]); }
  finally { clearTimeout(timer); }
}

// Node timers overflow above this range and would turn a long wait into 1 ms.
export function normalizeTurnWaitMs(value = 300_000) {
  if (value !== null && (!Number.isInteger(value) || value < 1 || value > 2_147_483_647)) {
    throw new Error('turnWaitMs must be null or an integer from 1 to 2147483647');
  }
  return value;
}

const utf8 = new TextDecoder('utf-8', { fatal: true }); // Never substitute U+FFFD for invalid bytes.
const STALE_REPLY = /unknown or finished turn|unknown or resolved interaction|No pending interaction/;

// Exact incoming stdout bytes are retained up to this fixed limit; exceeding it fails the session.
const STDOUT_CAPTURE_LIMIT = 64 * 1024 * 1024;

// onInteraction(record, signal) -> provider-native response. `record` is a copy of the record
// onItem saw (`record.data` is the complete interaction item, so the exact session_id, turn_id and
// data.request_id come from it). `signal` aborts when the turn finishes, the session fails or is
// cancelled, or the scope closes; a result returned after that is discarded and never sent.
export async function openRpc({ dueBinary, profile, artifactDir, onItem = () => {}, onInteraction, onFailure, signal, turnWaitMs }) {
  const waitMs = normalizeTurnWaitMs(turnWaitMs);
  if (onInteraction !== undefined && typeof onInteraction !== 'function') throw new Error('onInteraction must be a function');
  if (onFailure !== undefined && typeof onFailure !== 'function') throw new Error('onFailure must be a function');
  const notification = 'session.item';
  if (signal?.aborted) throw new Error('Session cancelled before launch');
  // Descriptors and the child are acquired together; a failure before launch completes releases only these.
  const opened = [];
  let wire, stderr, raw, child;
  try {
    for (const name of ['protocol.jsonl', 'stderr.log', 'stdout.bin']) opened.push(openSync(join(artifactDir, name), 'wx', 0o600));
    [wire, stderr, raw] = opened; // stdout.bin holds exact incoming bytes, before any decoding.
    child = spawn(dueBinary, ['rpc'], { shell: false, stdio: ['pipe', 'pipe', 'pipe'] });
  } catch (error) {
    for (const fd of opened) try { closeSync(fd); } catch {}
    throw error;
  }
  const exited = deferred(), pending = new Map(), turnIds = new Map(), decisions = new Map(), finishedTurns = new Set(), resolvedKeys = new Set();
  let sequence = 0, cursor = 0, sessionId, nativeId, active, fatal, closePromise, interruptPending, spawned = false;
  let provider = profile.provider, resolvedOptions = null;
  let closing = false, closed = false, buffer = Buffer.alloc(0), tail = Promise.resolve();
  let rawBytes = 0, rawOpen = true, queued = 0, queuedBytes = 0, wireBytes = 0, stderrBytes = 0, interrupted = false, wireOpen = true;
  const record = { pid: null, exit_code: null, signal: null, launch_error: null, forced_kill: false };
  function stop() { if (!interrupted && !closed && spawned) { interrupted = true; child.kill('SIGINT'); } }
  function abortDecisions(reason, turn) {
    for (const entry of decisions.values()) if (turn === undefined || entry.turn === turn) entry.control.abort(reason);
  }
  function fail(error) {
    const first = !fatal;
    fatal ??= error instanceof Error ? error : new Error(String(error));
    abortDecisions(fatal);
    for (const entry of pending.values()) { entry.collateral = true; entry.reject(fatal); }
    pending.clear(); active?.ready.reject(fatal); active?.done.reject(fatal); stop();
    // The scope uses this to cancel cooperative caller work, including an idle verifier.
    // A failure observer cannot replace the original transport/provider failure.
    if (first) { try { onFailure?.(fatal); } catch {} }
  }
  function rememberTurn(dueId, callerId) {
    turnIds.set(dueId, callerId);
    // Late events retain recent caller attribution without growing a direct client's map forever.
    while (turnIds.size > 64) {
      const retired = turnIds.keys().next().value;
      turnIds.delete(retired); finishedTurns.delete(retired);
      for (const key of resolvedKeys) if (JSON.parse(key)[0] === retired) resolvedKeys.delete(key);
    }
  }
  const abort = () => fail(new Error('Session cancelled'));
  signal?.addEventListener('abort', abort, { once: true });
  child.once('spawn', () => { spawned = true; record.pid = child.pid; if (signal?.aborted) abort(); });
  child.on('error', error => { record.launch_error = { code: error.code ?? null, message: error.message }; fail(error); });
  child.stdin.on('error', fail);
  child.stderr.on('data', chunk => {
    try { stderrBytes += chunk.length; if (stderrBytes > 8 * 1024 * 1024) throw new Error('Resident stderr exceeds 8 MiB'); writeAll(stderr, chunk); }
    catch (error) { fail(error); }
  });
  function capture(direction, message) {
    if (!wireOpen) return;
    const line = JSON.stringify({ direction, message }) + '\n';
    wireBytes += Buffer.byteLength(line);
    if (wireBytes > 64 * 1024 * 1024) throw new Error('Resident protocol evidence exceeds 64 MiB');
    writeAll(wire, line);
  }
  function send(message) {
    if (fatal) throw fatal;
    const line = JSON.stringify(message) + '\n';
    if (Buffer.byteLength(line) > 1024 * 1024) throw new Error('RPC request exceeds 1 MiB');
    capture('in', message);
    child.stdin.write(line, error => { if (error) fail(error); });
  }
  async function rpc(method, params, soft = false, meta = {}) {
    if (fatal) throw fatal;
    if (closed) throw new Error('Due RPC is already closed');
    const id = ++sequence, done = deferred(); pending.set(id, done);
    try {
      send({ jsonrpc: '2.0', id, method, params });
      return await bounded(done.promise, 300_000, `RPC ${method} timed out`);
    } catch (error) {
      meta.collateral = done.collateral === true; // Rejected by an earlier failure rather than failing itself.
      if (!soft || !error.remote) fail(error);
      throw error;
    }
    finally { pending.delete(id); }
  }
  function emit(wire) {
    if (!Number.isSafeInteger(wire.cursor) || wire.cursor < 1) throw new Error('Invalid session cursor');
    if (wire.cursor <= cursor) return;
    if (sessionId && wire.session_id !== sessionId) throw new Error('Foreign session item');
    cursor = wire.cursor;
    // Lifecycle and reply routing use this private copy; onItem may mutate the object it receives.
    const item = structuredClone(wire);
    if (item.kind === 'turn_started' && active && !active.dueId) {
      active.dueId = item.turn_id; rememberTurn(item.turn_id, active.id);
      active.ready.resolve(item.turn_id);
    }
    // Observers may request interruption; the terminal item must retire that option first.
    if (item.kind === 'turn_finished' && active?.dueId === item.turn_id) active.finished = true;
    // Only Claude's root system/init identifies the native session; nested or other payload IDs are ignored.
    let identityError;
    const root = item.data?.event;
    if (['native_summary', 'native_snapshot'].includes(item.kind) && item.data?.provider === 'claude'
      && root?.type === 'system' && root?.subtype === 'init' && typeof root.session_id === 'string' && root.session_id.trim()) {
      if (nativeId && nativeId !== root.session_id) identityError = new Error('Native session identity changed');
      else nativeId ??= root.session_id;
    }
    const observed = { source: 'native_session', turn_id: turnIds.get(item.turn_id) ?? null, data: wire };
    const callback = onItem(observed);
    if (callback?.then) {
      Promise.resolve(callback).catch(() => {});
      throw new Error('onItem must be synchronous');
    }
    if (identityError) throw identityError;
    // Rust pushes this when native exits between or during turns; keep the item, then fail the session.
    if (item.kind === 'session_failed') throw new Error(`Due session failed: ${item.data?.message ?? 'no message'}`);
    if (item.kind === 'omitted') throw new Error(`Native item omitted: ${JSON.stringify(item.data)}`);
    if (item.kind === 'interaction') {
      if (!onInteraction) throw new Error('Native interaction requires an explicit decision; this experiment does not auto-approve');
      decide({ source: 'native_session', turn_id: turnIds.get(item.turn_id) ?? null, data: item }, item);
    }
    if (item.kind === 'interaction_resolved' && typeof item.turn_id === 'string' && typeof item.data?.request_id === 'string') {
      // Abort only the decision for this exact turn and request; other requests are unaffected.
      const key = JSON.stringify([item.turn_id, item.data.request_id]);
      resolvedKeys.add(key);
      decisions.get(key)?.control.abort(new Error('Interaction resolved'));
    }
    if (item.kind === 'turn_finished') {
      finishedTurns.add(item.turn_id);
      abortDecisions(new Error('Turn finished'), item.turn_id); // A pending decision is now stale.
      if (active && item.turn_id === active.dueId) active.done.resolve(item.data);
    }
  }
  // Runs the caller's decision beside stdout parsing; nothing here blocks item delivery.
  function decide(observation, item) {
    if (fatal || closing || closed) return; // Never ask the caller about an interaction in a dead or closing session.
    const requestId = item.data?.request_id;
    if (typeof requestId !== 'string' || !requestId || typeof item.turn_id !== 'string' || typeof item.session_id !== 'string') {
      throw new Error('Native interaction lacks exact session, turn and request IDs');
    }
    const key = JSON.stringify([item.turn_id, requestId]);
    if (decisions.has(key)) return;
    const control = new AbortController();
    decisions.set(key, { control, turn: item.turn_id });
    const cancelled = () => control.signal.aborted || closing || closed;
    (async () => {
      let response;
      try { response = await onInteraction(structuredClone(observation), control.signal); }
      catch (error) {
        // Cooperative abort is not a failure; any other callback error is preserved even if it raced completion.
        if (!(cancelled() && (error === control.signal.reason || error?.name === 'AbortError'))) fail(error);
        return;
      }
      if (cancelled() || fatal) return; // Results after cancellation, completion or failure are discarded unsent.
      if (!response || typeof response !== 'object' || Array.isArray(response)) {
        return fail(new Error('onInteraction must return a provider-native response object'));
      }
      try {
        await rpc('interaction.reply', { session_id: item.session_id, turn_id: item.turn_id, request_id: requestId, response }, true);
      } catch (error) {
        // Only a recognized stale-reply rejection for this exact interaction's observed resolution, or
        // its turn's observed terminal item, is a benign race.
        if (error.remote && STALE_REPLY.test(error.message)) {
          try { await bounded(tail, 5_000, 'Resident item drain timed out'); } catch {}
          if (resolvedKeys.has(key) || finishedTurns.has(item.turn_id)) {
            try { capture('local', { event: 'stale_interaction_reply', turn_id: item.turn_id, request_id: requestId }); } catch {}
            return;
          }
        }
        fail(error);
      }
    })().catch(fail).finally(() => decisions.delete(key));
  }
  async function consume(item) {
    if (item.cursor > cursor + 1) {
      let page;
      do {
        page = await rpc('session.read', { session_id: item.session_id, after: cursor, limit: 64 });
        if (page.items_lost) throw new Error('Session journal lost requested items');
        const before = cursor;
        for (const restored of page.items) emit(restored);
        if (page.has_more && before === cursor) throw new Error('Session journal made no progress');
      } while (page.has_more);
    }
    emit(item);
  }
  child.stdout.on('data', chunk => {
    try {
      if (!rawOpen) return;
      const room = STDOUT_CAPTURE_LIMIT - rawBytes;
      writeAll(raw, chunk.length > room ? chunk.subarray(0, room) : chunk);
      rawBytes += Math.min(chunk.length, room);
      if (chunk.length > room) { rawOpen = false; throw new Error('Resident stdout capture exceeds 64 MiB'); }
      buffer = Buffer.concat([buffer, chunk]);
      if (buffer.length > 16 * 1024 * 1024) throw new Error('RPC frame exceeds 16 MiB');
      let end;
      while ((end = buffer.indexOf(10)) >= 0) {
        const message = JSON.parse(utf8.decode(buffer.subarray(0, end))); buffer = buffer.subarray(end + 1);
        capture('out', message);
        if (Object.hasOwn(message, 'id')) {
          const entry = pending.get(message.id);
          if (!entry) throw new Error('Unexpected RPC response ID');
          if (message.error) entry.reject(Object.assign(new Error(JSON.stringify(message.error)), { remote: true })); else entry.resolve(message.result);
        } else if (message.method === notification) {
          const item = message.params, bytes = Buffer.byteLength(JSON.stringify(item));
          queued += 1; queuedBytes += bytes;
          if (queued > 1024 || queuedBytes > 8 * 1024 * 1024) throw new Error('Resident notification buffer exhausted');
          tail = tail.then(() => consume(item)).catch(fail).finally(() => { queued -= 1; queuedBytes -= bytes; });
        }
      }
    } catch (error) { fail(error); }
  });
  child.stdout.on('end', () => {
    if (buffer.length) fail(new Error('Truncated RPC output frame'));
    else if (!closing) fail(new Error('Unexpected RPC stdout EOF'));
  });
  child.once('close', (code, term) => {
    closed = true; record.exit_code = spawned && Number.isInteger(code) ? code : null; record.signal = term;
    for (const entry of pending.values()) entry.reject(new Error('Due RPC closed before its response'));
    pending.clear();
    if (!closing && !fatal) fail(new Error(`Due RPC exited ${code ?? term}`));
    exited.resolve();
  });
  const getReceipt = () => ({ processes: [{ ...record }], native_session_id: nativeId ?? null,
    resolved_options: structuredClone(resolvedOptions), execution_error: fatal });
  async function close() {
    if (closePromise) return closePromise;
    closing = true;
    abortDecisions(new Error('Session closing')); // Do not wait for, or honor, a pending decision.
    active?.ready.reject(new Error('Session closed before the turn started'));
    // Release a turn waiting on a terminal item; this is not a session failure.
    active?.done.reject(Object.assign(new Error('Session closed before the turn finished'), { closedByCaller: true }));
    closePromise = (async () => {
      let cleanupError;
      const meta = {};
      try {
        if (!fatal && sessionId && !closed) await bounded(rpc('session.close', { session_id: sessionId }, false, meta), 20_000, 'session.close timed out');
      } catch (error) {
        // A close call rejected only because an earlier failure (kept as execution_error) cleared it is not
        // a cleanup failure. Its own errors, and exit status below, are still reported as cleanup.
        if (!meta.collateral) cleanupError = error;
        stop();
      }
      child.stdin.end();
      try { await bounded(exited.promise, 20_000, 'Due RPC did not close'); }
      catch (error) {
        cleanupError ??= error; record.forced_kill = true; child.kill('SIGKILL');
        await bounded(exited.promise, 5_000, 'Due RPC could not be reaped');
      } finally {
        // Child exit rejects any outstanding native control response. Join its local owner too.
        if (interruptPending) {
          if (!closed) fail(cleanupError ?? new Error('Session cleanup could not settle the control'));
          await interruptPending.operation.catch(() => {});
        }
        try { await bounded(tail, 5_000, 'Resident item drain timed out'); }
        catch (error) { cleanupError ??= error; fail(error); }
        signal?.removeEventListener('abort', abort);
        wireOpen = false; rawOpen = false; closeSync(wire); closeSync(stderr); closeSync(raw);
        jsonFile(join(artifactDir, 'resident-exit.json'), { ...record, error: (cleanupError ?? fatal)?.message ?? null });
      }
      if (cleanupError) throw cleanupError;
      if (record.exit_code !== 0 || record.signal) throw new Error(`Due RPC exited ${record.exit_code ?? record.signal}`);
      return getReceipt();
    })();
    return closePromise;
  }
  const backend = {
    capabilities: { nativeItems: true, interactionReply: Boolean(onInteraction), interrupt: true },
    interrupt({ id } = {}) {
      if (fatal) return Promise.reject(fatal);
      if (closing || closed) return Promise.reject(new Error('Session is closed'));
      const target = active;
      if (!target || target.id !== id || target.finished) return Promise.reject(new Error('No active turn with that caller ID'));
      if (interruptPending) return Promise.reject(new Error('An interruption is already pending'));
      if (target.interruptRequested) return Promise.reject(new Error('Interruption already requested for this turn'));
      target.interruptRequested = true;
      const entry = { target, operation: null };
      interruptPending = entry;
      entry.operation = (async () => {
        // Capture the exact turn now; startup and native acknowledgements can cross turn boundaries.
        const dueId = target.dueId ?? await target.ready.promise;
        if (fatal) throw fatal;
        if (closing || closed) throw new Error('Session is closed');
        if (target.finished || active !== target) throw new Error('Turn finished before interruption was sent');
        const acknowledged = await rpc('turn.interrupt', { session_id: sessionId, turn_id: dueId }, true);
        if (acknowledged?.interrupt_requested !== true || acknowledged.turn_id !== dueId) {
          const error = new Error('Interruption acknowledgement has an invalid turn identity');
          fail(error); throw error;
        }
        return acknowledged;
      })().finally(() => { if (interruptPending === entry) interruptPending = null; });
      entry.operation.catch(() => {});
      return entry.operation;
    },
    async turn({ id, prompt }) {
      if (fatal) throw fatal; // The recorded failure is more truthful than "closed" after a child exit.
      if (closing || closed) throw new Error('Session is closed');
      if (active) throw new Error('One turn at a time');
      if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,79}$/.test(id)) throw new Error('Invalid turn ID');
      const dir = join(artifactDir, id); mkdirSync(dir, { mode: 0o700 });
      jsonFile(join(dir, 'request.json'), { id, prompt });
      active = { id, dueId: null, done: deferred(), ready: deferred(), finished: false, interruptRequested: false };
      const target = active;
      try {
        const started = await rpc('turn.start', { session_id: sessionId, prompt });
        if (active.dueId && active.dueId !== started.turn_id) throw new Error('Turn identity changed');
        active.dueId = started.turn_id; rememberTurn(started.turn_id, id); active.ready.resolve(started.turn_id);
        const result = await (waitMs === null ? active.done.promise
          : bounded(active.done.promise, waitMs, 'Resident turn timed out waiting for its terminal item'));
        jsonFile(join(dir, 'result.json'), result);
        if (!result || result.schema_version !== 1 || result.provider !== provider
          || !['succeeded', 'failed', 'timed_out', 'cancelled'].includes(result.status)
          || (result.status === 'succeeded' && result.error !== null)) throw new Error('Invalid Due turn result');
        if (result.run_id != null && result.run_id !== started.turn_id) throw new Error('Due turn result identity changed');
        if (result.session_id != null && (typeof result.session_id !== 'string' || !result.session_id.trim())) throw new Error('Invalid native session ID');
        if (result.session_id) {
          if (nativeId && result.session_id !== nativeId) throw new Error('Native session identity changed');
          nativeId = result.session_id;
        } else if (result.status === 'succeeded') throw new Error('Successful turn has no native session ID');
        return { result, process: null, native_session_id: nativeId ?? null };
      } catch (error) {
        jsonFile(join(dir, 'error.json'), { message: error.message }); if (!error.closedByCaller) fail(error); throw error;
      } finally { target.ready.reject(new Error('Turn settled before startup')); active = null; }
    },
    close,
    getReceipt,
  };
  try {
    const opened = await rpc('session.open', { ...profile, stream: true });
    if (opened.protocol_version !== 1 || opened.stream?.enabled !== true
      || opened.stream?.method !== notification) throw new Error('Unsupported Due RPC session protocol');
    sessionId = opened.session_id;
    if (opened.provider !== undefined) {
      if (!['claude', 'codex'].includes(opened.provider) || (provider !== undefined && opened.provider !== provider)) {
        throw new Error('Resolved provider differs from session.open options');
      }
      provider = opened.provider;
    }
    if (!['claude', 'codex'].includes(provider)) throw new Error('Session open did not identify the resolved provider');
    if (opened.resolved_options !== undefined) {
      const options = opened.resolved_options;
      if (!options || typeof options !== 'object' || Array.isArray(options) || options.provider !== provider
        || options.stream !== true || typeof options.cwd !== 'string' || !options.cwd.trim()
        || typeof options.ephemeral !== 'boolean' || !Array.isArray(options.native_args)
        || !options.native_args.every(value => typeof value === 'string')) throw new Error('Invalid resolved session options');
      resolvedOptions = structuredClone(options);
    } else if (profile.request !== undefined) throw new Error('RPC server did not report resolved options for the request profile');
    if (opened.native_session_id) {
      if (nativeId && nativeId !== opened.native_session_id) throw new Error('Native session identity changed');
      nativeId = opened.native_session_id;
    }
    jsonFile(join(artifactDir, 'session-open.json'), opened);
    await tail;
    if (fatal) throw fatal;
    return backend;
  } catch (error) {
    fail(error);
    try { await close(); } catch (cleanup) { if (cleanup !== error) error.cleanup = cleanup.message; }
    error.sessionReceipt = getReceipt();
    throw error;
  }
}
