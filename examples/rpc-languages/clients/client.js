// Embedded Due RPC guest client (JavaScript), first slice.
//
// Runs inside a host interpreter that provides only `bridge` (send, recv, now_ms, log, args).
// No imports, filesystem, network, timers or Promises are used. Everything is SYNCHRONOUS and
// COOPERATIVE: frames are read only while a blocking method (poll, await, wait, open, start, turn,
// close) is running, and callbacks run inside that pump.
//
// Ordering inside an item: validate, snapshot lifecycle ids and terminal data (private structural
// copies), apply state transitions (retire interactions, cache the result, record fatal failures),
// THEN call onItem with the whole live native item, then onInteraction. Callbacks therefore cannot
// change what the client acts on, and cannot reply to a retired interaction or act after a recorded
// failure. A failing item (session_failed, omitted, invalid terminal result) is still delivered to
// onItem before the original failure is thrown.
//
// Callback rules:
//   * onItem(item) / onInteraction(item) must be synchronous. A returned thenable (or an async
//     function passed in) is rejected.
//   * Callbacks may call request / interrupt / reply (nonblocking). They must not call
//     poll / await / wait / open / start / turn / close; those throw a usage error. If the callback
//     lets that error escape, the callback failure is sticky like any other.
//   * onInteraction returns a provider-native response object (sent as interaction.reply and its
//     acknowledgement checked), or null/undefined to defer; the caller may reply() later. With no
//     onInteraction handler an interaction fails the session closed. Nothing is ever approved
//     implicitly.
//
// Failures:
//   * Transport, protocol (malformed or unexpected frames, unknown/duplicate response IDs, bad
//     envelope, bad item), callback, cursor/stream-loss and session_failed errors are STICKY: the
//     first cause is kept (`client.failure`) and every later call throws it.
//   * Ordinary JSON-RPC method errors (DueError kind 'rpc', with code/message/data), argument
//     (usage) errors and wait/await timeouts leave the client usable. open/start/close treat a
//     timeout as sticky because session state is then unknown.
//   * A turn whose DueResult.status is "failed" is returned as data, not thrown.
//
// Stream strictness (no recovery is implemented): items must arrive with cursors 1, 2, 3, ...
// A cursor gap, push_dropped > 0, an `omitted` item, an invalid cursor, a changed session_id or
// session_failed fails the client. Old/duplicate cursors (<= last seen) are ignored without being
// redelivered. `omitted` and `session_failed` items are passed to onItem first; gap and
// push_dropped items are rejected before delivery because they prove the stream is incomplete.
// Callers wanting recovery would have to use session.read; that is deliberately not done here.
//
// Numbers: frames are decoded with JSON.parse, so integers above 2^53 may be rounded in the decoded
// view. That view is not lossless; the host saves raw bytes before conversion. Outbound values with
// unsafe integers, non-finite numbers, undefined, functions, cycles, sparse arrays and non-plain
// objects are rejected rather than rewritten.
//
// Bounds: 64 pending requests, 64 tracked interactions, 64 cached turn results, 1 MiB outbound
// and 16 MiB inbound frames, depth 64 for outbound JSON.
var DueClient = (function () {
  'use strict';

  var MAX_PENDING = 64;
  var MAX_INTERACTIONS = 64;
  var MAX_TURNS = 64;
  var MAX_OUT_BYTES = 1024 * 1024;
  var MAX_IN_CHARS = 16 * 1024 * 1024;
  var MAX_DEPTH = 64;
  var MAX_TIMEOUT = 2147483647;
  var RECV_SLICE = 10000;
  var OPEN_TIMEOUT = 30000;
  var REQUEST_TIMEOUT = 30000;
  var CLOSE_TIMEOUT = 30000;
  var STATUSES = ['succeeded', 'failed', 'timed_out', 'cancelled'];

  function DueError(kind, message, extra) {
    var e = new Error(message);
    e.name = 'DueError';
    e.kind = kind;
    if (extra) {
      if (extra.cause !== undefined) e.cause = extra.cause;
      if (extra.code !== undefined) e.code = extra.code;
      if (extra.data !== undefined) e.data = extra.data;
    }
    return e;
  }

  function msg(e) {
    return e && typeof e === 'object' && typeof e.message === 'string' ? e.message : String(e);
  }
  function has(o, k) { return Object.prototype.hasOwnProperty.call(o, k); }
  function isObject(v) { return v !== null && typeof v === 'object' && !Array.isArray(v); }
  function isPlain(v) {
    if (!isObject(v)) return false;
    var p = Object.getPrototypeOf(v);
    return p === Object.prototype || p === null;
  }
  function isSafeCount(v) { return typeof v === 'number' && isFinite(v) && Math.floor(v) === v && Math.abs(v) <= 9007199254740991; }
  function nonEmptyString(v) { return typeof v === 'string' && v.length > 0; }
  function isThenable(v) {
    return v !== null && (typeof v === 'object' || typeof v === 'function') && typeof v.then === 'function';
  }
  function suspendingFunction(fn) {
    var c = fn.constructor && fn.constructor.name;
    return c === 'AsyncFunction' || c === 'AsyncGeneratorFunction' || c === 'GeneratorFunction';
  }

  function utf8Length(s) {
    var n = 0;
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (c < 0x80) n += 1;
      else if (c < 0x800) n += 2;
      else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length &&
               s.charCodeAt(i + 1) >= 0xdc00 && s.charCodeAt(i + 1) <= 0xdfff) { n += 4; i++; }
      else n += 3;
    }
    return n;
  }

  // Strict JSON encoder for outbound values. Throws a plain Error naming the offending path.
  function encode(value, path, stack) {
    var t = typeof value;
    if (value === null) return 'null';
    if (t === 'boolean') return value ? 'true' : 'false';
    if (t === 'string') return JSON.stringify(value);
    if (t === 'number') {
      if (!isFinite(value)) throw new Error(path + ': non-finite number');
      if (Math.floor(value) === value && Math.abs(value) > 9007199254740991) throw new Error(path + ': unsafe integer');
      if (value === 0 && 1 / value < 0) throw new Error(path + ': negative zero');
      return JSON.stringify(value);
    }
    if (t !== 'object') throw new Error(path + ': unsupported ' + t);
    if (stack.length >= MAX_DEPTH) throw new Error(path + ': nesting too deep');
    if (stack.indexOf(value) >= 0) throw new Error(path + ': cycle');
    stack.push(value);
    var out, i;
    if (Array.isArray(value)) {
      var parts = [];
      if (Object.getOwnPropertySymbols(value).length > 0) throw new Error(path + ': symbol keys');
      if (Object.getOwnPropertyNames(value).length !== value.length + 1) throw new Error(path + ': sparse array or extra array properties');
      for (i = 0; i < value.length; i++) {
        var ad = Object.getOwnPropertyDescriptor(value, i);
        if (!ad) throw new Error(path + '[' + i + ']: sparse array');
        if (!has(ad, 'value')) throw new Error(path + '[' + i + ']: accessor property');
        parts.push(encode(value[i], path + '[' + i + ']', stack));
      }
      out = '[' + parts.join(',') + ']';
    } else {
      if (!isPlain(value)) throw new Error(path + ': not a plain object');
      if (Object.getOwnPropertySymbols(value).length > 0) throw new Error(path + ': symbol keys');
      var keys = Object.keys(value), members = [];
      if (Object.getOwnPropertyNames(value).length !== keys.length) throw new Error(path + ': non-enumerable properties');
      for (i = 0; i < keys.length; i++) {
        if (!has(Object.getOwnPropertyDescriptor(value, keys[i]), 'value')) throw new Error(path + '.' + keys[i] + ': accessor property');
        members.push(JSON.stringify(keys[i]) + ':' + encode(value[keys[i]], path + '.' + keys[i], stack));
      }
      out = '{' + members.join(',') + '}';
    }
    stack.pop();
    return out;
  }

  function checkTimeout(v, dflt, name) {
    if (v === undefined) return dflt;
    if (typeof v !== 'number' || !isFinite(v) || Math.floor(v) !== v || v < 0 || v > MAX_TIMEOUT) {
      throw DueError('usage', name + ' must be an integer from 0 to ' + MAX_TIMEOUT);
    }
    return v;
  }

  function DueClientImpl(options) {
    if (!(this instanceof DueClientImpl)) throw DueError('usage', 'DueClient must be called with new');
    if (options === undefined) options = {};
    if (!isObject(options)) throw DueError('usage', 'options must be an object');
    var names = ['onItem', 'onInteraction'];
    for (var i = 0; i < names.length; i++) {
      var h = options[names[i]];
      if (h === undefined || h === null) continue;
      if (typeof h !== 'function') throw DueError('usage', names[i] + ' must be a function');
      if (suspendingFunction(h)) throw DueError('usage', names[i] + ' must be synchronous');
    }
    this._onItem = options.onItem || null;
    this._onInteraction = options.onInteraction || null;
    this.failure = null;
    this.closed = false;
    this._closing = false;
    this._closeResult = null;
    this._pumping = false;
    this._seq = 0;
    this._pending = new Map();      // id -> {method, done, result, error, auto}
    this._sessionId = null;         // from the open response
    this._itemSession = null;       // pinned from the first item (items can precede the response)
    this._cursor = 0;
    this._turns = new Map();        // turn_id -> DueResult | undefined (started, not finished)
    this._interactions = new Map(); // turn + request -> {turnId, requestId, answered}
  }

  var P = DueClientImpl.prototype;

  // ---- failure handling ----------------------------------------------------------------------

  // Records the FIRST cause and returns the error to throw. Later causes are discarded.
  P._fatal = function (kind, message, cause) {
    if (!this.failure) {
      this.failure = DueError(kind, message, { cause: cause });
      this._pending.clear();
      this._interactions.clear();
    }
    return this.failure;
  };
  P._guard = function () {
    if (this.failure) throw this.failure;
    if (this.closed) throw DueError('closed', 'client is closed');
  };
  P._live = function () { // for nonblocking operations
    this._guard();
    if (this._closing) throw DueError('usage', 'client is closing');
  };
  P._run = function (name, fn) {
    this._guard();
    if (this._pumping) {
      throw DueError('usage', name + ' is blocking and may not be called from a callback or while another blocking call runs');
    }
    this._pumping = true;
    try { return fn.call(this); } finally { this._pumping = false; }
  };
  P._callback = function (label, fn, arg) {
    var r;
    try { r = fn.call(undefined, arg); } catch (e) {
      if (e === this.failure) throw e;
      throw this._fatal('callback', label + ' threw: ' + msg(e), e);
    }
    var thenable;
    try { thenable = isThenable(r); } catch (e2) { throw this._fatal('callback', label + ' result inspection threw: ' + msg(e2), e2); }
    if (thenable) throw this._fatal('callback', label + ' returned a thenable; callbacks must be synchronous');
    return r;
  };

  // ---- outbound ------------------------------------------------------------------------------

  P._send = function (method, params, auto) {
    if (this._pending.size >= MAX_PENDING) throw DueError('usage', 'too many pending requests (max ' + MAX_PENDING + '); await earlier ones');
    var json = null;
    if (params !== undefined) {
      try { json = encode(params, 'params', []); } catch (e) { throw DueError('usage', 'invalid params: ' + msg(e), { cause: e }); }
    }
    var id = String(this._seq + 1);
    var frame = '{"jsonrpc":"2.0","id":' + JSON.stringify(id) + ',"method":' + JSON.stringify(method) +
      (json === null ? '' : ',"params":' + json) + '}';
    if (frame.length > MAX_OUT_BYTES || utf8Length(frame) > MAX_OUT_BYTES) throw DueError('usage', 'request exceeds 1 MiB');
    this._seq += 1;
    this._pending.set(id, { method: method, done: false, result: undefined, error: null, auto: auto === true });
    try { bridge.send(frame); } catch (e) { throw this._fatal('transport', 'bridge.send failed: ' + msg(e), e); }
    return id;
  };

  // ---- inbound -------------------------------------------------------------------------------

  // Reads one frame (<= ms, which must be <= 10000). Returns true when a frame was dispatched.
  P._pumpOnce = function (ms) {
    var frame;
    try { frame = bridge.recv(ms); } catch (e) { throw this._fatal('transport', 'bridge.recv failed: ' + msg(e), e); }
    if (frame === null) return false;
    if (typeof frame !== 'string') throw this._fatal('protocol', 'bridge.recv returned a non-string frame');
    try { this._handleFrame(frame); } catch (e2) {
      if (e2 === this.failure) throw e2;
      throw this._fatal('protocol', 'internal error handling frame: ' + msg(e2), e2);
    }
    return true;
  };

  P._pumpUntil = function (done, timeoutMs, onTimeout) {
    var deadline = bridge.now_ms() + timeoutMs;
    while (!done()) {
      var rem = deadline - bridge.now_ms();
      if (rem < 0) rem = 0;
      this._pumpOnce(rem > RECV_SLICE ? RECV_SLICE : rem);
      if (done()) return;
      if (bridge.now_ms() >= deadline) throw onTimeout();
    }
  };

  P._handleFrame = function (text) {
    if (text.length > MAX_IN_CHARS) throw this._fatal('protocol', 'inbound frame exceeds 16 MiB');
    var f;
    try { f = JSON.parse(text); } catch (e) { throw this._fatal('protocol', 'malformed JSON frame: ' + msg(e), e); }
    if (!isPlain(f)) throw this._fatal('protocol', 'frame is not a JSON object (batches are unsupported)');
    if (f.jsonrpc !== '2.0') throw this._fatal('protocol', 'frame lacks jsonrpc "2.0"');
    var keys = Object.keys(f), i;
    if (has(f, 'method')) {
      if (has(f, 'id')) throw this._fatal('protocol', 'unexpected server-to-client request');
      if (f.method !== 'session.item') throw this._fatal('protocol', 'unexpected notification ' + JSON.stringify(f.method));
      for (i = 0; i < keys.length; i++) {
        if (keys[i] !== 'jsonrpc' && keys[i] !== 'method' && keys[i] !== 'params') throw this._fatal('protocol', 'unknown notification field ' + keys[i]);
      }
      if (!isPlain(f.params)) throw this._fatal('protocol', 'session.item params must be an object');
      this._handleItem(f.params);
      return;
    }
    for (i = 0; i < keys.length; i++) {
      if (keys[i] !== 'jsonrpc' && keys[i] !== 'id' && keys[i] !== 'result' && keys[i] !== 'error') throw this._fatal('protocol', 'unknown response field ' + keys[i]);
    }
    if (typeof f.id !== 'string') throw this._fatal('protocol', 'response id is not one of this client\'s string ids');
    if (has(f, 'result') === has(f, 'error')) throw this._fatal('protocol', 'response must contain exactly one of result and error');
    var entry = this._pending.get(f.id);
    if (!entry) throw this._fatal('protocol', 'response for unknown id ' + f.id);
    if (entry.done) throw this._fatal('protocol', 'duplicate response for id ' + f.id);
    if (has(f, 'error')) {
      var er = f.error;
      if (!isPlain(er) || !isSafeCount(er.code) || typeof er.message !== 'string') throw this._fatal('protocol', 'malformed error object for id ' + f.id);
      entry.error = er;
    } else {
      entry.result = f.result;
    }
    entry.done = true;
    if (entry.auto) { // acknowledgement of an automatic interaction.reply: checked, then released
      this._pending.delete(f.id);
      if (entry.error) {
        throw this._fatal('rpc', 'interaction.reply rejected (' + entry.error.code + '): ' + entry.error.message,
          DueError('rpc', entry.error.message, { code: entry.error.code, data: entry.error.data }));
      }
    }
  };

  P._handleItem = function (p) {
    var sid = p.session_id, cursor = p.cursor, kind = p.kind, turnId = p.turn_id, dropped = p.push_dropped;
    if (!nonEmptyString(sid)) throw this._fatal('protocol', 'item has no session_id');
    if (!isSafeCount(cursor) || cursor < 1) throw this._fatal('protocol', 'item has an invalid cursor');
    if (!nonEmptyString(kind)) throw this._fatal('protocol', 'item has no kind');
    if (turnId !== undefined && turnId !== null && !nonEmptyString(turnId)) throw this._fatal('protocol', 'item turn_id is invalid');
    if (dropped !== undefined && (!isSafeCount(dropped) || dropped < 0)) throw this._fatal('protocol', 'item push_dropped is invalid');
    if (this._sessionId) {
      if (sid !== this._sessionId) throw this._fatal('protocol', 'item session_id changed');
    } else if (this._itemSession) {
      if (sid !== this._itemSession) throw this._fatal('protocol', 'item session_id changed');
    } else {
      this._itemSession = sid;
    }
    if (cursor <= this._cursor) return; // old duplicate: ignored, not redelivered
    if (cursor !== this._cursor + 1) {
      throw this._fatal('stream', 'item cursor gap: expected ' + (this._cursor + 1) + ', received ' + cursor + '; recovery is not implemented');
    }
    if (dropped > 0) throw this._fatal('stream', 'push_dropped ' + dropped + ' at cursor ' + cursor + '; recovery is not implemented');
    this._cursor = cursor;

    // Snapshot every lifecycle fact and make every state transition BEFORE any callback runs.
    // onItem receives the live decoded item and may mutate or inspect it freely; nothing below
    // reads p again. Terminal data is cloned structurally (never through the outbound encoder).
    var data = p.data;
    var requestId = isObject(data) ? data.request_id : undefined;
    var entry = null, forInteraction = null;
    if (kind === 'turn_finished') {
      this._finishTurn(turnId, data);
    } else if (kind === 'interaction') {
      entry = this._trackInteraction(turnId, requestId);
      if (entry && this._onInteraction) forInteraction = this._cloneItem(p);
    } else if (kind === 'interaction_resolved') {
      if (typeof turnId === 'string' && nonEmptyString(requestId)) this._interactions.delete(ikey(turnId, requestId));
    } else if (kind === 'session_failed') {
      // Failure is recorded first so callbacks can no longer act; the item is still delivered below.
      this._fatal('session_failed', 'Due session failed: ' + (isObject(data) && typeof data.message === 'string' ? data.message : 'no message'));
    } else if (kind === 'omitted') {
      this._fatal('stream', 'native item omitted at cursor ' + cursor + '; the stream is not complete');
    }

    // The whole native item always reaches onItem, including items that already fail the client.
    if (this._onItem) this._callback('onItem', this._onItem, p);
    if (this.failure) throw this.failure;

    if (entry && !entry.answered) { // onItem may already have answered via reply()
      if (!this._onInteraction) throw this._fatal('policy', 'interaction ' + entry.requestId + ' needs an explicit decision and no onInteraction handler exists; failing closed');
      var response = this._callback('onInteraction', this._onInteraction, forInteraction);
      if (this.failure) throw this.failure;
      if (response === null || response === undefined) return; // deferred; caller may reply() later
      if (entry.answered) throw this._fatal('callback', 'onInteraction both replied and returned a response');
      this._answer(entry, sid, response, true);
    }
  };

  // Structural deep copy of decoded JSON (arrays, plain objects, primitives). Own keys are defined
  // rather than assigned so a decoded "__proto__" key stays plain data.
  function cloneJson(v, depth) {
    if (v === null || typeof v !== 'object') return v;
    if (depth > 512) throw new Error('payload nesting too deep');
    var out, i;
    if (Array.isArray(v)) {
      out = [];
      for (i = 0; i < v.length; i++) out.push(cloneJson(v[i], depth + 1));
      return out;
    }
    out = {};
    var keys = Object.keys(v);
    for (i = 0; i < keys.length; i++) {
      Object.defineProperty(out, keys[i], { value: cloneJson(v[keys[i]], depth + 1), enumerable: true, writable: true, configurable: true });
    }
    return out;
  }
  function ikey(turnId, requestId) { return JSON.stringify([turnId, requestId]); }

  P._cloneItem = function (p) {
    try { return cloneJson(p, 0); } catch (e) { throw this._fatal('protocol', 'cannot copy item: ' + msg(e), e); }
  };

  // Validates and caches a terminal result, retiring that turn's interactions. Never throws for a
  // bad result: it records the failure and the caller still delivers the item before throwing it.
  P._finishTurn = function (turnId, raw) {
    if (typeof turnId !== 'string') { this._fatal('protocol', 'turn_finished without turn_id'); return; }
    var r;
    try { r = cloneJson(raw, 0); } catch (e) { this._fatal('protocol', 'cannot copy DueResult: ' + msg(e), e); return; }
    if (!isObject(r) || r.schema_version !== 1 || STATUSES.indexOf(r.status) < 0 ||
        (r.status === 'succeeded' && r.error !== null && r.error !== undefined)) {
      this._fatal('protocol', 'invalid DueResult in turn_finished'); return;
    }
    if (r.run_id !== undefined && r.run_id !== null && r.run_id !== turnId) { this._fatal('protocol', 'DueResult run_id differs from turn_id'); return; }
    if (this._turns.get(turnId) !== undefined) { this._fatal('protocol', 'duplicate turn_finished for ' + turnId); return; }
    if (!this._turns.has(turnId) && this._turns.size >= MAX_TURNS) {
      this._fatal('protocol', 'finished-turn cache full (' + MAX_TURNS + '); call wait() for finished turns'); return;
    }
    this._turns.set(turnId, r);
    this._retireTurn(turnId);
  };

  // Registers a pending interaction before callbacks run. Returns null when it is not tracked
  // (closing, already failed, or invalid; invalid ones fail the client).
  P._trackInteraction = function (turnId, requestId) {
    if (this._closing || this.failure) return null;
    if (typeof turnId !== 'string' || !nonEmptyString(requestId)) { this._fatal('protocol', 'interaction lacks exact turn and request ids'); return null; }
    var key = ikey(turnId, requestId);
    if (this._interactions.has(key)) { this._fatal('protocol', 'duplicate interaction ' + requestId); return null; }
    if (this._interactions.size >= MAX_INTERACTIONS) { this._fatal('protocol', 'too many pending interactions'); return null; }
    var entry = { turnId: turnId, requestId: requestId, answered: false };
    this._interactions.set(key, entry);
    return entry;
  };

  P._retireTurn = function (turnId) {
    var self = this;
    this._interactions.forEach(function (v, k) { if (v.turnId === turnId) self._interactions.delete(k); });
  };

  P._answer = function (entry, sessionId, response, auto) {
    if (!isPlain(response)) {
      var bad = DueError('usage', 'interaction response must be a native response object');
      if (auto) throw this._fatal('callback', bad.message, bad);
      throw bad;
    }
    var params = { session_id: sessionId, turn_id: entry.turnId, request_id: entry.requestId, response: response };
    var id;
    try { id = this._send('interaction.reply', params, auto); } catch (e) {
      if (auto && e !== this.failure) throw this._fatal('callback', 'cannot send interaction.reply: ' + msg(e), e);
      throw e;
    }
    entry.answered = true;
    return id;
  };

  // ---- public API ----------------------------------------------------------------------------

  P.request = function (method, params) {
    this._live();
    if (!nonEmptyString(method)) throw DueError('usage', 'method must be a non-empty string');
    if (params !== undefined && !isPlain(params)) throw DueError('usage', 'params must be an object');
    return this._send(method, params, false);
  };

  function finish(self, id, entry) {
    self._pending.delete(id);
    if (entry.error) {
      throw DueError('rpc', 'RPC error ' + entry.error.code + ': ' + entry.error.message,
        { code: entry.error.code, data: entry.error.data });
    }
    return entry.result;
  }

  P._awaitId = function (id, timeoutMs, stickyTimeout, label) {
    var self = this, entry = this._pending.get(id);
    this._pumpUntil(function () { return entry.done; }, timeoutMs, function () {
      var m = (label || 'await ' + id) + ' timed out after ' + timeoutMs + ' ms';
      return stickyTimeout ? self._fatal('timeout', m) : DueError('timeout', m);
    });
    return finish(this, id, entry);
  };

  P.await = function (id, timeoutMs) {
    timeoutMs = checkTimeout(timeoutMs, 30000, 'timeoutMs');
    return this._run('await', function () {
      var entry = typeof id === 'string' ? this._pending.get(id) : undefined;
      if (!entry) throw DueError('usage', 'unknown request id');
      if (entry.auto) throw DueError('usage', 'request id belongs to an automatic interaction reply');
      return this._awaitId(id, timeoutMs, false);
    });
  };

  // Nonblocking settlement hooks for the optional Promise scheduler. They reuse
  // the same decoder, identity checks and lifecycle transitions as sync calls.
  P._asyncTake = function (id) {
    this._guard();
    var entry = this._pending.get(id);
    if (!entry || entry.auto) throw DueError('usage', 'unknown request id');
    return entry.done ? { ready: true, value: finish(this, id, entry) } : { ready: false };
  };
  P._asyncOpen = function (r) {
    if (!isObject(r) || r.protocol_version !== 1 || !isObject(r.stream) || r.stream.enabled !== true ||
        r.stream.method !== 'session.item' || !nonEmptyString(r.session_id)) {
      throw this._fatal('protocol', 'unsupported session.open result');
    }
    if (this._itemSession && this._itemSession !== r.session_id) throw this._fatal('protocol', 'session_id differs from earlier items');
    this._sessionId = r.session_id;
    return r;
  };
  P._asyncStart = function (r) {
    if (!isObject(r) || !nonEmptyString(r.turn_id)) throw this._fatal('protocol', 'turn.start result lacks turn_id');
    if (!this._turns.has(r.turn_id)) {
      if (this._turns.size >= MAX_TURNS) throw this._fatal('protocol', 'finished-turn cache full');
      this._turns.set(r.turn_id, undefined);
    }
    return r.turn_id;
  };
  P._asyncWait = function (id) {
    this._guard();
    if (!this._turns.has(id)) throw DueError('usage', 'unknown turn id');
    var value = this._turns.get(id);
    if (value === undefined) return { ready: false };
    this._turns.delete(id);
    return { ready: true, value: value };
  };
  P._asyncBeginClose = function () {
    this._guard();
    this._closing = true;
    this._interactions.clear();
    return this._send('session.close', { session_id: this._sessionId }, false);
  };
  P._asyncClosed = function (result) {
    this._pending.clear();
    this._turns.clear();
    this.closed = true;
    this._closeResult = result;
    return result;
  };

  P.poll = function (timeoutMs) {
    timeoutMs = checkTimeout(timeoutMs, 0, 'timeoutMs');
    return this._run('poll', function () {
      var deadline = bridge.now_ms() + timeoutMs;
      for (;;) {
        var rem = deadline - bridge.now_ms();
        if (rem < 0) rem = 0;
        if (this._pumpOnce(rem > RECV_SLICE ? RECV_SLICE : rem)) return true;
        if (bridge.now_ms() >= deadline) return false;
      }
    });
  };

  P.open = function (profile) {
    return this._run('open', function () {
      if (this._sessionId) throw DueError('usage', 'a session is already open; this client serves one session');
      if (!isPlain(profile)) throw DueError('usage', 'profile must be an object');
      var params = {}, keys = Object.keys(profile);
      for (var i = 0; i < keys.length; i++) params[keys[i]] = profile[keys[i]];
      params.stream = true;
      var r = this._awaitId(this._send('session.open', params, false), OPEN_TIMEOUT, true, 'session.open');
      if (!isObject(r) || r.protocol_version !== 1 || !isObject(r.stream) || r.stream.enabled !== true ||
          r.stream.method !== 'session.item' || !nonEmptyString(r.session_id)) {
        throw this._fatal('protocol', 'unsupported session.open result (protocol_version 1 with session.item streaming required)');
      }
      if (this._itemSession && this._itemSession !== r.session_id) throw this._fatal('protocol', 'session_id differs from earlier items');
      this._sessionId = r.session_id;
      return r;
    });
  };

  P.start = function (prompt) {
    return this._run('start', function () {
      if (!this._sessionId) throw DueError('usage', 'no open session');
      if (this._closing) throw DueError('usage', 'client is closing');
      if (typeof prompt !== 'string') throw DueError('usage', 'prompt must be a string');
      if (this._turns.size >= MAX_TURNS) throw DueError('usage', 'too many unwaited turns');
      var r = this._awaitId(this._send('turn.start', { session_id: this._sessionId, prompt: prompt }, false),
        REQUEST_TIMEOUT, true, 'turn.start');
      if (!isObject(r) || !nonEmptyString(r.turn_id)) throw this._fatal('protocol', 'turn.start result lacks turn_id');
      if (!this._turns.has(r.turn_id)) {
        if (this._turns.size >= MAX_TURNS) throw this._fatal('protocol', 'finished-turn cache full');
        this._turns.set(r.turn_id, undefined); // otherwise its terminal item already arrived
      }
      return r.turn_id;
    });
  };

  P.wait = function (turnId, timeoutMs) {
    timeoutMs = checkTimeout(timeoutMs, 300000, 'timeoutMs');
    return this._run('wait', function () {
      var self = this;
      if (typeof turnId !== 'string' || !this._turns.has(turnId)) throw DueError('usage', 'unknown turn id');
      this._pumpUntil(function () { return self._turns.get(turnId) !== undefined; }, timeoutMs, function () {
        return DueError('timeout', 'turn ' + turnId + ' did not finish within ' + timeoutMs + ' ms'); // still running; may wait again
      });
      var result = this._turns.get(turnId);
      this._turns.delete(turnId);
      return result;
    });
  };

  // timeoutMs bounds the wait for the terminal result (turn.start has its own fixed 30 s bound).
  P.turn = function (prompt, timeoutMs) {
    timeoutMs = checkTimeout(timeoutMs, 300000, 'timeoutMs');
    return this.wait(this.start(prompt), timeoutMs);
  };

  P.interrupt = function (turnId) {
    this._live();
    if (!this._sessionId) throw DueError('usage', 'no open session');
    if (!nonEmptyString(turnId)) throw DueError('usage', 'turnId must be a non-empty string');
    return this._send('turn.interrupt', { session_id: this._sessionId, turn_id: turnId }, false);
  };

  P.reply = function (interactionItem, response) {
    this._live();
    if (!isObject(interactionItem) || interactionItem.kind !== 'interaction' || !isObject(interactionItem.data)) {
      throw DueError('usage', 'reply requires an interaction item');
    }
    var turnId = interactionItem.turn_id, requestId = interactionItem.data.request_id;
    if (typeof turnId !== 'string' || !nonEmptyString(requestId) || interactionItem.session_id !== (this._sessionId || this._itemSession)) {
      throw DueError('usage', 'interaction item has inconsistent identifiers');
    }
    var entry = this._interactions.get(ikey(turnId, requestId));
    if (!entry) throw DueError('usage', 'interaction is not pending (resolved, finished, retired or unknown)');
    if (entry.answered) throw DueError('usage', 'interaction was already answered');
    return this._answer(entry, this._sessionId || this._itemSession, response, false);
  };

  P.close = function () {
    if (this.closed) return this._closeResult;
    return this._run('close', function () {
      if (!this._sessionId) { this.closed = true; return null; }
      this._closing = true;
      this._interactions.clear(); // no replies once closing
      var r;
      try {
        r = this._awaitId(this._send('session.close', { session_id: this._sessionId }, false), CLOSE_TIMEOUT, true, 'session.close');
      } catch (e) {
        if (e === this.failure) throw e;
        throw this._fatal('rpc', 'session.close failed: ' + msg(e), e);
      }
      // Items before the response were dispatched in order; the host owns draining and process join.
      this._pending.clear();
      this._turns.clear();
      this.closed = true;
      this._closeResult = r;
      return r;
    });
  };

  var g = typeof globalThis !== 'undefined' ? globalThis : undefined;
  if (g) { g.DueError = DueError; }
  return DueClientImpl;
})();
if (typeof globalThis !== 'undefined') globalThis.DueClient = DueClient;
