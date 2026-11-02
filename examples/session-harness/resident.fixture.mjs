#!/usr/bin/env node
// Synthetic persistent peer only; no native providers or model calls.
import { createInterface } from 'node:readline';
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const plain = process.argv[2] === 'rpc';
if (!['mcp', 'rpc'].includes(process.argv[2])) throw new Error('Unknown transport');
let directory, mode, count = 0, cursor = 0, journal = [], current;
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const result = (id, value) => send({ jsonrpc: '2.0', id, result: value });
const tool = (id, value) => result(id, plain ? value : { structuredContent: value, content: [{ type: 'text', text: JSON.stringify(value) }] });
const item = (kind, data, turn_id = null, push = true) => {
  const record = { session_id: 'due-session', cursor: ++cursor, turn_id, kind, data };
  journal.push(record);
  if (push) send({ jsonrpc: '2.0', method: plain ? 'session.item' : 'notifications/due/session_item', params: { ...record, push_dropped: 0 } });
  return record;
};
const outcome = turn => ({ schema_version: 1, provider: 'codex', run_id: mode === 'wrong-run' ? 'stale-turn' : turn,
  session_id: mode === 'missing-id' ? null : 'native-one', status: mode === 'unknown-status' ? 'finished' : mode === 'failed' ? 'failed' : 'succeeded',
  text: `turn ${count}`, error: ['failed', 'false-success'].includes(mode) ? { kind: 'provider', message: 'synthetic failure' } : null });
const input = createInterface({ input: process.stdin });
input.on('line', line => {
  const request = JSON.parse(line);
  if (directory) appendFileSync(join(directory, 'fixture-input.jsonl'), line + '\n');
  if (!plain && request.method === 'initialize') return result(request.id, { protocolVersion: '2025-11-25' });
  if (!plain && request.method !== 'tools/call') return;
  const { name, arguments: args } = plain
    ? { name: request.method.replace('.', '_'), arguments: request.params } : request.params;
  if (name === 'session_open') {
    directory = args.cwd; mode = args.model;
    appendFileSync(join(directory, 'fixture-input.jsonl'), line + '\n');
    item('session_opened', {});
    return tool(request.id, { session_id: 'due-session', native_session_id: 'native-one',
      ...(plain ? { protocol_version: mode === 'wrong-version' ? 9 : 1, stream: { enabled: true, method: 'session.item' } } : {}) });
  }
  if (name === 'session_read') {
    const items = journal.filter(value => value.cursor > args.after).slice(0, 64);
    return tool(request.id, { items, cursor: items.at(-1)?.cursor ?? args.after, items_lost: mode === 'lost', has_more: false });
  }
  if (name === 'turn_start') {
    if (mode === 'rpc-error') {
      send({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Fixture request rejected' } }); return;
    }
    current = `native-turn-${++count}`;
    item('turn_started', { prompt: args.prompt }, current);
    if (mode === 'malformed') { process.stdout.write('not-json\n'); return; }
    if (mode === 'hold') { tool(request.id, { turn_id: current }); return; }
    if (mode === 'interaction') {
      item('interaction', { request_id: 'permission', request: { command: 'must-not-run' } }, current);
      tool(request.id, { turn_id: current }); return;
    }
    item('native', { provider: 'codex', method: 'item/completed', params: { item: { nested: { future: [null, 3, true] } } } }, current, !['gap', 'lost'].includes(mode));
    item('turn_finished', outcome(current), current);
    // Native stream and terminal deliberately precede tools/call response.
    tool(request.id, { turn_id: current });
  }
  if (name === 'session_close') {
    tool(request.id, { closed: true });
    // After response, still deliver a final complete session item.
    item('session_closed', {});
  }
});
function finish() {
  if (directory) writeFileSync(join(directory, 'fixture-exited'), 'closed\n');
  process.stdout.end(() => process.exit(mode === 'cleanup-failure' ? 7 : 0));
}
input.on('close', finish);
process.on('SIGINT', finish);
