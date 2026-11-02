// One logical conversation, with a fresh Due/native process for each turn.
import { spawn } from 'node:child_process';
import { closeSync, openSync, writeFileSync, writeSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { StringDecoder } from 'node:string_decoder';

const MAX_RESULT = 8 * 1024 * 1024;
const MAX_TRACE_LINE = 1024 * 1024;
const asError = value => {
  if (value instanceof Error) return value;
  try { return new Error(String(value)); }
  catch { return new Error('Non-Error value thrown'); }
};
const describe = value => {
  const error = asError(value);
  return { code: error.code ?? null, message: error.message };
};
function writeAll(fd, chunk) {
  for (let offset = 0; offset < chunk.length;) {
    const written = writeSync(fd, chunk, offset, chunk.length - offset);
    if (written === 0) throw new Error('Output file write made no progress');
    offset += written;
  }
}

export async function openResume({ dueBinary = 'due', profile, artifactDir, onItem = () => {}, signal }) {
  const capturedProfile = structuredClone(profile);
  const processes = [];
  let nativeId = null, attempted = false, unavailable = null, closed = false, active = null;

  function cancel(state) {
    state.cancelled = true;
    if (state.spawned && !state.exited && !state.forwarded) {
      state.forwarded = true;
      state.child.kill('SIGINT');
    }
  }

  async function execute(state, { id, prompt }) {
    const dir = join(artifactDir, id);
    const request = { schema_version: 1, ...capturedProfile, prompt, ephemeral: false };
    if (nativeId) request.resume = nativeId;
    const args = ['run', '--request', '-', '--output', 'json', '--trace=json'];
    const receipt = {
      command: dueBinary, args, pid: null, exit_code: null, signal: null,
      started_at: new Date().toISOString(), finished_at: null,
      launch_error: null, stdin_error: null, collection_error: null, cancel_requested: false,
    };
    const descriptors = [];
    let bytes = 0, chunks = [], traceLine = '', skipping = false, failure = null;
    const decoder = new StringDecoder('utf8');
    const fail = value => {
      const error = asError(value);
      failure ??= error;
      receipt.collection_error ??= describe(error);
      cancel(state);
    };
    const emitLine = line => {
      let record;
      try { record = JSON.parse(line); } catch { return; } // Diagnostics remain in stderr.log.
      if (record?.schema_version === 1 && typeof record.event === 'string' && typeof record.run_id === 'string') {
        try {
          const returned = onItem({ source: 'due_trace', turn_id: id, data: record });
          if (returned?.then) {
            Promise.resolve(returned).catch(() => {});
            fail(new Error('onItem must be synchronous'));
          }
        } catch (error) { fail(error); }
      }
    };
    const trace = text => {
      // A long diagnostic is retained on disk without becoming an unbounded parser buffer.
      for (const part of text.split(/(?<=\n)/)) {
        if (!skipping) {
          traceLine += part;
          if (Buffer.byteLength(traceLine) > MAX_TRACE_LINE) { skipping = true; traceLine = ''; }
        }
        if (part.endsWith('\n')) {
          if (!skipping) emitLine(traceLine.trimEnd());
          traceLine = ''; skipping = false;
        }
      }
    };
    const file = name => {
      const fd = openSync(join(dir, name), 'wx', 0o600);
      descriptors.push(fd);
      return fd;
    };
    const aborted = () => cancel(state);
    try {
      await mkdir(dir, { mode: 0o700 }); // Reserve even failed attempts; never overwrite evidence.
      writeFileSync(file('request.json'), `${JSON.stringify(request, null, 2)}\n`);
      const out = file('result.json'), err = file('stderr.log'), exit = file('exit.json');
      signal?.addEventListener('abort', aborted, { once: true });
      if (signal?.aborted) cancel(state);
      if (state.cancelled) throw new Error('Session cancelled before process launch');
      attempted = true;
      await new Promise(resolve => {
        const child = spawn(dueBinary, args, { shell: false, stdio: ['pipe', 'pipe', 'pipe'] });
        state.child = child;
        child.once('spawn', () => {
          state.spawned = true;
          receipt.pid = child.pid;
          if (state.cancelled) cancel(state);
        });
        child.on('error', error => { receipt.launch_error = describe(error); failure ??= error; });
        child.stdin.on('error', error => { receipt.stdin_error = describe(error); failure ??= error; cancel(state); });
        child.stdout.on('data', chunk => {
          try { writeAll(out, chunk); } catch (error) { fail(error); }
          bytes += chunk.length;
          if (bytes <= MAX_RESULT) chunks.push(chunk);
          else if (chunks !== null) { chunks = null; fail(new Error('Due result exceeds 8 MiB parsing limit')); }
        });
        child.stderr.on('data', chunk => {
          try { writeAll(err, chunk); trace(decoder.write(chunk)); } catch (error) { fail(error); }
        });
        child.stdout.on('error', fail);
        child.stderr.on('error', fail);
        child.once('close', (code, terminationSignal) => {
          state.exited = true;
          receipt.exit_code = state.spawned && Number.isInteger(code) ? code : null;
          receipt.signal = terminationSignal;
          trace(decoder.end());
          if (traceLine && !skipping) emitLine(traceLine);
          resolve();
        });
        child.stdin.end(JSON.stringify(request));
      });
      receipt.cancel_requested = state.cancelled;
      receipt.finished_at = new Date().toISOString();
      writeFileSync(exit, `${JSON.stringify(receipt, null, 2)}\n`);
      processes.push({ ...receipt });
      if (failure) throw failure;
      let result;
      try { result = JSON.parse(Buffer.concat(chunks).toString('utf8')); }
      catch (error) { throw new Error(`Due returned invalid result JSON: ${error.message}`); }
      if (result?.schema_version !== 1 || result.provider !== capturedProfile.provider
          || !['succeeded', 'failed', 'timed_out', 'cancelled'].includes(result.status)) {
        throw new Error('Due returned an unexpected result envelope');
      }
      if (result.status === 'succeeded' && (receipt.exit_code !== 0 || receipt.signal || result.error != null)) {
        throw new Error('Successful result disagrees with Due process outcome');
      }
      const observedId = typeof result.session_id === 'string' && result.session_id.trim() ? result.session_id : null;
      if (observedId && nativeId && observedId !== nativeId) throw new Error('Native session identity changed across resume');
      if (observedId) nativeId = observedId;
      if (result.status === 'succeeded' && !observedId) throw new Error('Successful turn omitted its native session ID');
      if (!nativeId) unavailable = 'Previous turn supplied no resumable native session ID';
      return { result, process: { pid: receipt.pid, exit_code: receipt.exit_code, signal: receipt.signal }, native_session_id: nativeId };
    } catch (error) {
      unavailable = error.message;
      // Setup/parse failures still have their original request/output/exit files.
      if (descriptors.length && !receipt.finished_at) {
        receipt.cancel_requested = state.cancelled;
        receipt.finished_at = new Date().toISOString();
        receipt.collection_error ??= describe(error);
        writeFileSync(join(dir, 'exit.json'), `${JSON.stringify(receipt, null, 2)}\n`, { mode: 0o600 });
      }
      throw error;
    } finally {
      signal?.removeEventListener('abort', aborted);
      for (const fd of descriptors) closeSync(fd);
    }
  }

  return {
    capabilities: Object.freeze({ nativeItems: false, interactionReply: false, interrupt: false }),
    turn(job) {
      if (closed || signal?.aborted) return Promise.reject(new Error('Session is closed or cancelled'));
      if (active) return Promise.reject(new Error('A turn is already active'));
      if (unavailable || (attempted && !nativeId)) return Promise.reject(new Error(unavailable ?? 'No resumable session ID'));
      if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,79}$/.test(job.id) || typeof job.prompt !== 'string' || !job.prompt.trim()) {
        return Promise.reject(new Error('Turn requires a safe id and nonempty prompt'));
      }
      const state = { spawned: false, exited: false, forwarded: false, cancelled: false };
      active = state;
      state.done = execute(state, job).finally(() => { active = null; });
      return state.done;
    },
    async close() {
      closed = true;
      if (active) {
        const state = active;
        cancel(state);
        // The turn promise retains its own error; close still joins the process.
        await state.done.catch(() => {});
      }
      return { processes: processes.map(value => ({ ...value })), native_session_id: nativeId };
    },
  };
}
