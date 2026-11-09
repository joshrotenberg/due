// Drive promises backed by the in-process core engine.
var Due = (() => {
  'use strict';
  const bridge = __dueCore;
  const codec = __dueJson;
  const pending = new Map();
  const state = { started: false, done: false, error: null };
  let result = 'null';

  function fail(error) {
    state.error ??= typeof error?.message === 'string' ? error.message : String(error);
    state.done = true;
  }

  async function submit(op, params) {
    if (!state.started || state.done) throw new Error('workflow is closed');
    const id = bridge.start(codec.encode({ op, params }, 'operation'));
    return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
  }

  globalThis.__dueTick = () => {
    for (let count = 0; count < 16 && !state.done; count++) {
      const frame = bridge.poll();
      if (frame == null) break;
      const { id, result } = JSON.parse(frame);
      const operation = pending.get(id);
      if (!operation) throw new Error('unknown core completion');
      pending.delete(id);
      if (Object.prototype.hasOwnProperty.call(result, 'Err')) {
        const error = new Error(result.Err.message);
        error.kind = result.Err.kind;
        error.data = result.Err.data;
        operation.reject(error);
      } else {
        operation.resolve(result.Ok);
      }
    }
  };
  globalThis.__dueState = () => JSON.stringify(state);
  globalThis.__dueFail = fail;
  globalThis.__dueResult = () => result;

  const scope = Object.freeze({
    spawn(provider, prompt, timeoutMs) {
      return submit('spawn', { provider, prompt, ...(timeoutMs === undefined ? {} : { timeout_ms: timeoutMs }) });
    },
    followUp(session, prompt, timeoutMs) {
      return submit('follow_up', { session, prompt, ...(timeoutMs === undefined ? {} : { timeout_ms: timeoutMs }) });
    },
    inspect(run) { return submit('inspect', { run }); },
    wait(runs, timeoutMs = 1000) { return submit('wait', { runs, timeout_ms: timeoutMs }); },
    interrupt(run) { return submit('interrupt', { run }); },
    respond(interaction, response) { return submit('respond', { interaction, response }); },
    readItems(session, after = 0, limit = 32) { return submit('read_items', { session, after, limit }); },
  });

  return Object.freeze({
    args: JSON.parse(bridge.args),
    log: bridge.log,
    run(body) {
      if (state.started) throw new Error('Due.run may be called once');
      if (typeof body !== 'function') throw new Error('Due.run requires a function');
      state.started = true;
      Promise.resolve().then(() => body(scope)).then(value => {
        if (pending.size) throw new Error('workflow returned with unjoined core operations');
        result = codec.result(value);
        state.done = true;
      }).catch(fail);
    },
  });
})();
