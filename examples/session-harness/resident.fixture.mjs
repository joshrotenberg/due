#!/usr/bin/env node
// Synthetic persistent peer only; no native providers or model calls.
import { createInterface } from 'node:readline';
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const plain = process.argv[2] === 'rpc';
if (!['mcp', 'rpc'].includes(process.argv[2])) throw new Error('Unknown transport');
let directory, mode, count = 0, cursor = 0, journal = [], current, held;
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
  session_id: mode === 'missing-id' ? null : mode.startsWith('init') ? 'claude-native' : 'native-one', status: mode === 'unknown-status' ? 'finished' : mode === 'failed' ? 'failed' : 'succeeded',
  text: `turn ${count}`, error: ['failed', 'false-success'].includes(mode) ? { kind: 'provider', message: 'synthetic failure' } : null });
const reject = (id, message) => plain
  ? send({ jsonrpc: '2.0', id, error: { code: -32000, message } })
  : result(id, { isError: true, content: [{ type: 'text', text: message }] });
const finishTurn = () => item('turn_finished', outcome(current), current);
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
    return tool(request.id, { session_id: 'due-session', native_session_id: mode.startsWith('init') ? null : 'native-one',
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
    if (mode.startsWith('init')) {
      const claude = event => item('native_summary', { provider: 'claude', event }, current);
      // Ignored: nested IDs, non-init system events and non-Claude providers.
      item('native_summary', { provider: 'codex', event: { type: 'system', subtype: 'init', session_id: 'codex-native' } }, current);
      claude({ type: 'system', subtype: 'status', session_id: 'status-native' });
      claude({ type: 'system', subtype: 'init', nested: { session_id: 'nested-native' } });
      item('native', { provider: 'claude', event: { type: 'assistant', session_id: 'assistant-native' } }, current);
      if (mode === 'init-unrelated') { process.stdout.write('not-json\n'); return; }
      claude({ type: 'system', subtype: 'init', session_id: 'claude-native' });
      if (mode === 'init-conflict') claude({ type: 'system', subtype: 'init', session_id: 'other-native' });
      if (mode === 'init-malformed') { process.stdout.write('not-json\n'); return; }
      if (mode === 'init-hold') { tool(request.id, { turn_id: current }); return; }
    }
    if (mode === 'malformed') { process.stdout.write('not-json\n'); return; }
    if (mode === 'hold') { tool(request.id, { turn_id: current }); return; }
    if (mode === 'wait-delayed') {
      tool(request.id, { turn_id: current });
      setTimeout(finishTurn, 120);
      return;
    }
    if (mode.startsWith('interaction')) {
      item('interaction', { request_id: 'permission', request: { command: 'must-not-run' } }, current);
      // A later item must keep flowing while the decision is pending.
      item('native', { provider: 'codex', method: 'fixture/during-decision', params: {} }, current);
      // Resolution of this request, or of an unrelated one, while the turn stays active.
      if (mode === 'interaction-resolved') { item('interaction_resolved', { request_id: 'permission' }, current); setTimeout(finishTurn, 150); }
      if (mode.startsWith('interaction-unrelated')) item('interaction_resolved', { request_id: 'other' }, current);
      // 'interaction-finished': the turn ends without any reply, making the decision stale.
      if (mode === 'interaction-finished') finishTurn();
      tool(request.id, { turn_id: current }); return;
    }
    item('native', { provider: 'codex', method: 'item/completed', params: { item: { nested: { future: [null, 3, true] } } } }, current, !['gap', 'lost'].includes(mode));
    item('turn_finished', outcome(current), current);
    // Native stream and terminal deliberately precede tools/call response.
    tool(request.id, { turn_id: current });
  }
  if (name === 'interaction_reply') {
    if (mode === 'interaction-reject') return reject(request.id, 'Fixture rejected reply');
    if (mode === 'interaction-stale-genuine') return reject(request.id, 'unknown or finished turn');
    if (args.session_id !== 'due-session' || args.turn_id !== current || args.request_id !== 'permission') {
      return reject(request.id, 'unknown or resolved interaction');
    }
    if (mode === 'interaction-resolved-race') { item('interaction_resolved', { request_id: 'permission' }, current); setTimeout(finishTurn, 100); return reject(request.id, 'unknown or resolved interaction'); }
    if (mode === 'interaction-unrelated-race') { return reject(request.id, 'unknown or resolved interaction'); }
    if (mode === 'interaction-late-error-on-close') { finishTurn(); held = request.id; return; }
    if (mode === 'interaction-late-error') { finishTurn(); return reject(request.id, 'permissions backend unavailable'); }
    if (mode === 'interaction-race') { finishTurn(); return reject(request.id, 'unknown or finished turn'); }
    item('native', { provider: 'codex', method: 'fixture/reply', params: args.response }, current);
    finishTurn();
    return tool(request.id, { replied: true, request_id: 'permission' });
  }
  if (name === 'session_close' && held) {
    // The genuine reply failure arrives while close is outstanding; close completes only afterwards.
    reject(held, 'permissions backend unavailable'); held = null;
    setTimeout(() => { tool(request.id, { closed: true }); item('session_closed', {}); }, 200);
    return;
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
