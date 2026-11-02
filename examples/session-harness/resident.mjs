// Optional Node 22+ persistent backends. Both transports share lifecycle and recovery.
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

export const openResident = options => openPersistent(options, 'mcp');
export const openRpc = options => openPersistent(options, 'rpc');

// Node timers overflow above this range and would turn a long wait into 1 ms.
export function normalizeTurnWaitMs(value = 300_000) {
  if (value !== null && (!Number.isInteger(value) || value < 1 || value > 2_147_483_647)) {
    throw new Error('turnWaitMs must be null or an integer from 1 to 2147483647');
  }
  return value;
}

const STALE_REPLY = /unknown or finished turn|unknown or resolved interaction|No pending interaction/;

// onInteraction(record, signal) -> provider-native response. `record` is a copy of the record
// onItem saw (`record.data` is the complete interaction item, so the exact session_id, turn_id and
// data.request_id come from it). `signal` aborts when the turn finishes, the session fails or is
// cancelled, or the scope closes; a result returned after that is discarded and never sent.
async function openPersistent({ dueBinary, profile, artifactDir, onItem = () => {}, onInteraction, signal, turnWaitMs }, transport) {
  const waitMs = normalizeTurnWaitMs(turnWaitMs);
  if (onInteraction !== undefined && typeof onInteraction !== 'function') throw new Error('onInteraction must be a function');
  const label = transport === 'rpc' ? 'RPC' : 'MCP';
  const notification = transport === 'rpc' ? 'session.item' : 'notifications/due/session_item';
  if (signal?.aborted) throw new Error('Session cancelled before launch');
  const wire = openSync(join(artifactDir, 'protocol.jsonl'), 'wx', 0o600);
  const stderr = openSync(join(artifactDir, 'stderr.log'), 'wx', 0o600);
  const child = spawn(dueBinary, [transport], { shell: false, stdio: ['pipe', 'pipe', 'pipe'] });
  const exited = deferred(), pending = new Map(), turnIds = new Map(), decisions = new Map(), finishedTurns = new Set(), resolvedKeys = new Set();
  let sequence = 0, cursor = 0, sessionId, nativeId, active, fatal, closePromise, spawned = false;
  let closing = false, closed = false, buffer = Buffer.alloc(0), tail = Promise.resolve();
  let queued = 0, queuedBytes = 0, wireBytes = 0, stderrBytes = 0, interrupted = false, wireOpen = true;
  const record = { pid: null, exit_code: null, signal: null, launch_error: null, forced_kill: false };
  function stop() { if (!interrupted && !closed && spawned) { interrupted = true; child.kill('SIGINT'); } }
  function abortDecisions(reason, turn) {
    for (const entry of decisions.values()) if (turn === undefined || entry.turn === turn) entry.control.abort(reason);
  }
  function fail(error) {
    fatal ??= error instanceof Error ? error : new Error(String(error));
    abortDecisions(fatal);
    for (const entry of pending.values()) { entry.collateral = true; entry.reject(fatal); }
    pending.clear(); active?.done.reject(fatal); stop();
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
    if (Buffer.byteLength(line) > 1024 * 1024) throw new Error(`${label} request exceeds 1 MiB`);
    capture('in', message);
    child.stdin.write(line, error => { if (error) fail(error); });
  }
  async function rpc(method, params, soft = false, meta = {}) {
    if (fatal) throw fatal;
    if (closed) throw new Error(`Due ${label} is already closed`);
    const id = ++sequence, done = deferred(); pending.set(id, done);
    try {
      send({ jsonrpc: '2.0', id, method, params });
      return await bounded(done.promise, 300_000, `${label} ${method} timed out`);
    } catch (error) {
      meta.collateral = done.collateral === true; // Rejected by an earlier failure rather than failing itself.
      if (!soft || !error.remote) fail(error);
      throw error;
    }
    finally { pending.delete(id); }
  }
  async function call(name, args, soft = false, meta = {}) {
    if (transport === 'rpc') return rpc(name.replace('_', '.'), args, soft, meta);
    const value = await rpc('tools/call', { name, arguments: args }, soft, meta);
    if (value.isError) throw Object.assign(new Error(`${name}: ${JSON.stringify(value.content)}`), { remote: true });
    if (!value.structuredContent) throw new Error(`${name}: missing structured result`);
    return value.structuredContent;
  }
  function emit(wire) {
    if (!Number.isSafeInteger(wire.cursor) || wire.cursor < 1) throw new Error('Invalid session cursor');
    if (wire.cursor <= cursor) return;
    if (sessionId && wire.session_id !== sessionId) throw new Error('Foreign session item');
    cursor = wire.cursor;
    // Lifecycle and reply routing use this private copy; onItem may mutate the object it receives.
    const item = structuredClone(wire);
    if (item.kind === 'turn_started' && active && !active.dueId) {
      active.dueId = item.turn_id; turnIds.set(item.turn_id, active.id);
    }
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
        await call('interaction_reply', { session_id: item.session_id, turn_id: item.turn_id, request_id: requestId, response }, true);
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
        page = await call('session_read', { session_id: item.session_id, after: cursor, limit: 64 });
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
      buffer = Buffer.concat([buffer, chunk]);
      if (buffer.length > 16 * 1024 * 1024) throw new Error(`${label} frame exceeds 16 MiB`);
      let end;
      while ((end = buffer.indexOf(10)) >= 0) {
        const message = JSON.parse(buffer.subarray(0, end).toString('utf8')); buffer = buffer.subarray(end + 1);
        capture('out', message);
        if (Object.hasOwn(message, 'id')) {
          const entry = pending.get(message.id);
          if (!entry) throw new Error(`Unexpected ${label} response ID`);
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
    if (buffer.length) fail(new Error(`Truncated ${label} output frame`));
    else if (!closing) fail(new Error(`Unexpected ${label} stdout EOF`));
  });
  child.once('close', (code, term) => {
    closed = true; record.exit_code = spawned && Number.isInteger(code) ? code : null; record.signal = term;
    for (const entry of pending.values()) entry.reject(new Error(`Due ${label} closed before its response`));
    pending.clear();
    if (!closing && !fatal) fail(new Error(`Due ${label} exited ${code ?? term}`));
    exited.resolve();
  });
  const getReceipt = () => ({ processes: [{ ...record }], native_session_id: nativeId ?? null, execution_error: fatal });
  async function close() {
    if (closePromise) return closePromise;
    closing = true;
    abortDecisions(new Error('Session closing')); // Do not wait for, or honor, a pending decision.
    // Release a turn waiting on a terminal item; this is not a session failure.
    active?.done.reject(Object.assign(new Error('Session closed before the turn finished'), { closedByCaller: true }));
    closePromise = (async () => {
      let cleanupError;
      const meta = {};
      try {
        if (!fatal && sessionId && !closed) await bounded(call('session_close', { session_id: sessionId }, false, meta), 20_000, 'session_close timed out');
      } catch (error) {
        // A close call rejected only because an earlier failure (kept as execution_error) cleared it is not
        // a cleanup failure. Its own errors, and exit status below, are still reported as cleanup.
        if (!meta.collateral) cleanupError = error;
        stop();
      }
      child.stdin.end();
      try { await bounded(exited.promise, 20_000, `Due ${label} did not close`); }
      catch (error) {
        cleanupError ??= error; record.forced_kill = true; child.kill('SIGKILL');
        await bounded(exited.promise, 5_000, `Due ${label} could not be reaped`);
      } finally {
        try { await bounded(tail, 5_000, 'Resident item drain timed out'); }
        catch (error) { cleanupError ??= error; fail(error); }
        signal?.removeEventListener('abort', abort);
        wireOpen = false; closeSync(wire); closeSync(stderr);
        jsonFile(join(artifactDir, 'resident-exit.json'), { ...record, error: (cleanupError ?? fatal)?.message ?? null });
      }
      if (cleanupError) throw cleanupError;
      if (record.exit_code !== 0 || record.signal) throw new Error(`Due ${label} exited ${record.exit_code ?? record.signal}`);
      return getReceipt();
    })();
    return closePromise;
  }
  const backend = {
    capabilities: { nativeItems: true, interactionReply: Boolean(onInteraction), interrupt: false },
    async turn({ id, prompt }) {
      if (closing || closed) throw new Error('Session is closed');
      if (fatal) throw fatal;
      if (active) throw new Error('One turn at a time');
      if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,79}$/.test(id)) throw new Error('Invalid turn ID');
      const dir = join(artifactDir, id); mkdirSync(dir, { mode: 0o700 });
      jsonFile(join(dir, 'request.json'), { id, prompt });
      active = { id, dueId: null, done: deferred() };
      try {
        const started = await call('turn_start', { session_id: sessionId, prompt });
        if (active.dueId && active.dueId !== started.turn_id) throw new Error('Turn identity changed');
        active.dueId = started.turn_id; turnIds.set(started.turn_id, id);
        const result = await (waitMs === null ? active.done.promise
          : bounded(active.done.promise, waitMs, 'Resident turn timed out waiting for its terminal item'));
        jsonFile(join(dir, 'result.json'), result);
        if (!result || result.schema_version !== 1 || result.provider !== profile.provider
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
      } finally { active = null; }
    },
    close,
    getReceipt,
  };
  try {
    if (transport === 'mcp') {
      await rpc('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'due-session-harness', version: '1' } });
      send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    }
    const opened = await call('session_open', { ...profile, ephemeral: false, stream: true });
    if (transport === 'rpc' && (opened.protocol_version !== 1 || opened.stream?.enabled !== true
      || opened.stream?.method !== notification)) throw new Error('Unsupported Due RPC session protocol');
    sessionId = opened.session_id;
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
