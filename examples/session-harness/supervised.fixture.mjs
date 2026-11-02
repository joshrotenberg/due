#!/usr/bin/env node
// Synthetic resident worker. It changes a disposable artifact; no provider/model calls.
import { appendFileSync, writeFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { join } from 'node:path';

if (process.argv[2] !== 'rpc') throw new Error('Expected rpc');
let cwd, provider, mode, turn = 0, cursor = 0;
const journal = [];
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
const reply = (id, result) => send({ jsonrpc: '2.0', id, result });
const item = (kind, data, turn_id = null) => {
  const record = { session_id: 'supervised-session', turn_id, cursor: ++cursor, kind, data };
  journal.push(record);
  send({ jsonrpc: '2.0', method: 'session.item', params: { ...record, push_dropped: 0 } });
};
const input = createInterface({ input: process.stdin });
input.on('line', line => {
  const { id, method, params } = JSON.parse(line);
  if (method === 'session.open') {
    ({ cwd, provider, model: mode } = params);
    appendFileSync(join(cwd, 'launches.jsonl'), `${JSON.stringify({ pid: process.pid })}\n`);
    return reply(id, { session_id: 'supervised-session', provider, native_session_id: 'supervised-native',
      protocol_version: 1, capabilities: { interrupt: true }, stream: { enabled: true, method: 'session.item' } });
  }
  appendFileSync(join(cwd, 'calls.jsonl'), `${line}\n`);
  if (method === 'turn.start') {
    const turnId = `turn-${++turn}`;
    item('turn_started', { prompt: params.prompt }, turnId);
    if (mode === 'transport-error') {
      return send({ jsonrpc: '2.0', id, error: { code: -32000, message: 'synthetic transport failure' } });
    }
    const fixed = mode === 'correct' || (mode !== 'never-fix' && turn > 1);
    writeFileSync(join(cwd, 'answer.txt'), fixed ? '42\n' : '41\n');
    item('native', { provider, method: 'item/completed', params: { text: 'All tests pass.' } }, turnId);
    item('turn_finished', { schema_version: 1, provider, run_id: turnId, session_id: 'supervised-native',
      status: mode === 'worker-failed' ? 'failed' : 'succeeded', text: 'Implemented. All tests pass.',
      error: mode === 'worker-failed' ? { kind: 'provider', message: 'synthetic worker failure' } : null }, turnId);
    return reply(id, { turn_id: turnId });
  }
  if (method === 'session.read') {
    return reply(id, { items: journal.filter(item => item.cursor > params.after), cursor,
      latest_cursor: cursor, has_more: false, items_lost: false });
  }
  if (method === 'session.close') {
    item('session_closed', { native_session_id: 'supervised-native', error: null });
    return reply(id, { closed: true, native_session_id: 'supervised-native' });
  }
  throw new Error(`Unexpected method ${method}`);
});
function finish() {
  if (cwd) writeFileSync(join(cwd, 'exited'), 'yes');
  process.stdout.end(() => process.exit(0));
}
input.on('close', finish);
process.once('SIGINT', finish);
