// Optional Promise supervision over the synchronous client's validated protocol.
// Rust advances QuickJS jobs and calls __dueTick; callbacks never block that pump.
var Due = (function () {
  'use strict';
  const state = { started: false, done: false, error: null };
  const requests = new Map(), waits = new Map(), commands = new Set(), timers = new Set();
  const callbacks = new Map(), callbackTasks = new Set(), barriers = new Set();
  let client, worker, bodyDone = false, shutting = false, shutdownAt = 0;
  let opening = false, active = null, starting = false, interrupted = null;
  let options = {}, sessionController;
  const message = e => e && typeof e.message === 'string' ? e.message : String(e);
  const error = e => e instanceof Error ? e : new Error(message(e));
  const copy = value => client ? client._cloneItem(value) : value;

  function controller() {
    let aborted = false, reason;
    const signal = Object.freeze({
      get aborted() { return aborted; }, get reason() { return reason; },
      throwIfAborted() { if (aborted) throw reason; },
    });
    return { signal, abort(value) { if (!aborted) { aborted = true; reason = error(value); } } };
  }
  function checkSignal(signal) {
    if (!signal || typeof signal.throwIfAborted !== 'function' || typeof signal.aborted !== 'boolean') {
      throw new Error('signal must be a Due scope or interaction signal');
    }
    signal.throwIfAborted();
  }
  function fail(value) {
    state.error ||= error(value);
    sessionController?.abort(state.error);
    for (const entry of callbackTasks) entry.control.abort(state.error);
  }
  function live() {
    if (state.error) throw state.error;
    if (shutting || state.done) throw new Error('async scope is closed');
    sessionController.signal.throwIfAborted();
  }
  function until(test) {
    return new Promise(resolve => { barriers.add({ test, resolve }); });
  }
  function rpc(id, owner, timeout = 30000) {
    return new Promise((resolve, reject) => {
      requests.set(id, { resolve, reject, owner, deadline: bridge.now_ms() + timeout });
    });
  }
  function request(method, params, owner) { return rpc(client.request(method, params), owner); }
  function key(item) { return JSON.stringify([item.turn_id, item.data.request_id]); }
  function retired(entry) {
    return entry.control.signal.aborted || shutting || state.error || client.failure;
  }
  function observe(item) {
    const snapshot = copy(item);
    if (snapshot.kind === 'interaction_resolved') {
      callbacks.get(key(snapshot))?.control.abort(new Error('Interaction resolved'));
    } else if (snapshot.kind === 'turn_finished') {
      for (const entry of callbackTasks) {
        if (entry.item.turn_id === snapshot.turn_id) entry.control.abort(new Error('Turn finished'));
      }
    } else if (snapshot.kind === 'session_failed' || snapshot.kind === 'omitted') {
      fail(client.failure || new Error('Due session failed'));
    }
    if (options.onItem) {
      const returned = options.onItem(item);
      if (returned && typeof returned.then === 'function') {
        throw new Error('onItem must be synchronous; use onInteraction for awaited decisions');
      }
    }
  }
  function decide(item) {
    if (state.error || shutting) return null;
    if (typeof options.onInteraction !== 'function') throw new Error('Interaction needs an explicit decision');
    if (callbackTasks.size >= 64) throw new Error('too many pending permission callbacks');
    const identity = key(item), control = controller();
    const entry = { item: copy(item), control };
    callbacks.set(identity, entry);
    callbackTasks.add(entry);
    const ownedWork = () => [...commands, ...timers].some(work => work.signal === control.signal);
    const finished = () => {
      if (ownedWork()) {
        const e = new Error('permission callback returned with unjoined work');
        if (!control.signal.aborted) fail(e);
        control.abort(e);
      }
      if (callbacks.get(identity) === entry) callbacks.delete(identity);
      callbackTasks.delete(entry);
    };
    Promise.resolve().then(() => {
      control.signal.throwIfAborted();
      return options.onInteraction(copy(entry.item), control.signal);
    }).then(async response => {
      if (retired(entry)) return;
      if (ownedWork()) throw new Error('permission callback returned with unjoined work');
      if (!response || typeof response !== 'object' || Array.isArray(response)) {
        throw new Error('onInteraction must return a native response object');
      }
      try { await rpc(client.reply(entry.item, response), entry); }
      catch (e) {
        const stale = e && e.kind === 'rpc' && ['unknown or finished turn', 'unknown or resolved interaction', 'No pending interaction']
          .some(message => e.message === 'RPC error ' + e.code + ': ' + message);
        if (!(stale && retired(entry))) throw e;
      }
    }).then(finished, e => {
      if (!(control.signal.aborted && e === control.signal.reason)) fail(e);
      finished();
    });
    return null; // The base client retains the exact interaction for explicit reply.
  }

  function exec(spec, signal) {
    live(); signal ||= sessionController.signal; checkSignal(signal);
    const id = bridge.exec_start(JSON.stringify(spec));
    return new Promise((resolve, reject) => commands.add({ id, signal, resolve, reject, cancelled: false }));
  }
  function sleep(ms, signal) {
    live(); signal ||= sessionController.signal; checkSignal(signal);
    if (!Number.isInteger(ms) || ms < 0 || ms > 300000) throw new Error('sleep must be 0..300000 milliseconds');
    if (timers.size >= 64) throw new Error('too many pending sleeps');
    return new Promise((resolve, reject) => timers.add({ deadline: bridge.now_ms() + ms, signal, resolve, reject }));
  }

  async function open(profile, handlers = {}) {
    live();
    if (client || opening) throw new Error('one session per async scope');
    for (const name of ['onItem', 'onInteraction']) {
      if (handlers[name] !== undefined && typeof handlers[name] !== 'function') throw new Error(name + ' must be a function');
    }
    options = handlers; opening = true;
    client = new DueClient({ onItem: observe, onInteraction: decide });
    try {
      const opened = client._asyncOpen(await request('session.open', { ...profile, stream: true }));
      worker = Object.freeze({
        opened: copy(opened), signal: sessionController.signal,
        async start(prompt) {
          live();
          if (starting || active) throw new Error('one active turn per session');
          if (typeof prompt !== 'string' || !prompt.trim()) throw new Error('prompt must be nonempty');
          starting = true;
          try {
            const id = client._asyncStart(await request('turn.start', { session_id: client._sessionId, prompt }));
            active = id; // The reservation lasts until wait consumes its terminal result.
            return id;
          } finally { starting = false; }
        },
        wait(id, timeout = 300000) {
          live();
          if (!Number.isInteger(timeout) || timeout <= 0 || timeout > 2147483647) throw new Error('invalid turn wait timeout');
          if (waits.has(id)) throw new Error('a wait for this turn already exists');
          if (!client._turns.has(id)) throw new Error('unknown turn id');
          return new Promise((resolve, reject) => waits.set(id, { resolve, reject, deadline: bridge.now_ms() + timeout }));
        },
        async turn(prompt, timeout) { return worker.wait(await worker.start(prompt), timeout); },
        async interrupt(id) {
          live();
          if (active !== id || !id || client._turns.get(id) !== undefined) throw new Error('no active turn with that id');
          if (interrupted) throw new Error('an interruption is already pending');
          interrupted = id;
          try {
            const result = await rpc(client.interrupt(id));
            if (!result || result.interrupt_requested !== true || result.turn_id !== id) throw new Error('Invalid interruption acknowledgement');
            return result;
          } finally { interrupted = null; }
        },
        reply(item, response) { live(); return rpc(client.reply(item, response)); },
        close() { return closeSession(); },
      });
      return worker;
    } catch (e) { fail(e); throw e; }
    finally { opening = false; }
  }
  let closingSession;
  function closeSession() {
    if (closingSession) return closingSession;
    if (!client || client.closed || !client._sessionId) return Promise.resolve(null);
    for (const entry of callbackTasks) entry.control.abort(new Error('Session closed'));
    closingSession = (async () => {
      if (client.failure) throw client.failure;
      const result = await rpc(client._asyncBeginClose(), 'cleanup');
      // Retirement suppresses new replies, but sent replies still own their acknowledgements.
      await until(() => ![...requests.values()].some(entry => entry.owner && entry.owner !== 'cleanup'));
      const closed = client._asyncClosed(result);
      active = null;
      return closed;
    })();
    return closingSession;
  }
  async function shutdown() {
    shutting = true; shutdownAt = bridge.now_ms();
    if (starting || active || opening || waits.size || (client && client._turns.size)
      || [...requests.values()].some(entry => !entry.owner)
      || [...commands].some(entry => entry.signal === sessionController.signal)
      || [...timers].some(entry => entry.signal === sessionController.signal)) {
      fail(new Error('scope returned with an active turn or unjoined work'));
    }
    sessionController.abort(state.error || new Error('Scope closed'));
    for (const entry of callbackTasks) entry.control.abort(state.error || new Error('Scope closed'));
    try { await closeSession(); } catch (e) { fail(e); }
    await until(() => !commands.size && !timers.size && !callbackTasks.size && !requests.size && !waits.size);
    state.done = true;
  }
  function tick() {
    if (!state.started || state.done) return;
    if (client && !client.closed && !client.failure) {
      try { for (let i = 0; i < 32 && client.poll(0); i++); } catch (e) { fail(e); }
    }
    const now = bridge.now_ms();
    for (const [id, entry] of requests) {
      try {
        if (state.error && entry.owner !== 'cleanup') throw state.error;
        if (client.failure) throw client.failure;
        if (now > entry.deadline) { const e = new Error('RPC acknowledgement timed out'); fail(e); throw e; }
        const settled = client._asyncTake(id);
        if (settled.ready) { requests.delete(id); entry.resolve(settled.value); }
      } catch (e) { requests.delete(id); entry.reject(e); }
    }
    for (const [id, entry] of waits) {
      try {
        if (state.error) throw state.error;
        const settled = client._asyncWait(id);
        if (settled.ready) {
          waits.delete(id);
          if (active === id) active = null;
          entry.resolve(settled.value);
        } else if (now >= entry.deadline) {
          const e = new Error('worker turn wait timed out'); fail(e); throw e;
        }
      } catch (e) { waits.delete(id); entry.reject(e); }
    }
    for (const entry of commands) {
      try {
        const cancelled = entry.signal.aborted ? entry.signal : sessionController.signal.aborted ? sessionController.signal : null;
        if (cancelled && !entry.cancelled) { bridge.exec_cancel(entry.id); entry.cancelled = true; }
        const raw = bridge.exec_poll(entry.id);
        if (raw !== null) {
          commands.delete(entry);
          if (cancelled) entry.reject(cancelled.reason);
          else entry.resolve(JSON.parse(raw));
        }
      } catch (e) { commands.delete(entry); entry.reject(e); fail(e); }
    }
    for (const entry of timers) {
      const cancelled = entry.signal.aborted ? entry.signal : sessionController.signal.aborted ? sessionController.signal : null;
      if (cancelled || now >= entry.deadline) {
        timers.delete(entry);
        if (cancelled) entry.reject(cancelled.reason); else entry.resolve();
      }
    }
    for (const entry of barriers) {
      if (entry.test()) { barriers.delete(entry); entry.resolve(); }
    }
    if (bodyDone && !shutting) shutdown().catch(e => { fail(e); state.done = true; });
    if (shutting && !state.done && now - shutdownAt > 10000) {
      fail(new Error('async scope cleanup did not settle within 10 seconds'));
      state.done = true;
    }
  }
  const api = Object.freeze({
    run(body) {
      if (state.started) throw new Error('Due.run may be called once');
      if (typeof body !== 'function') throw new Error('Due.run requires a function');
      state.started = true; sessionController = controller();
      const scope = Object.freeze({ signal: sessionController.signal, open, exec, sleep, cancel: fail });
      Promise.resolve().then(() => body(scope)).then(() => { bodyDone = true; }, e => { fail(e); bodyDone = true; });
    },
  });
  globalThis.__dueTick = tick;
  globalThis.__dueState = () => JSON.stringify({ started: state.started, done: state.done,
    error: state.error ? message(state.error) : null });
  globalThis.__dueFail = value => { fail(value); bodyDone = true; };
  return api;
})();
