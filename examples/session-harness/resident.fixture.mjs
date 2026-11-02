#!/usr/bin/env node
// Synthetic persistent peer only; no native providers or model calls.
import { createInterface } from 'node:readline';
import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
if (process.argv[2] !== 'rpc') throw new Error('Unknown transport');
let directory, mode, count = 0, cursor = 0, journal = [], current, held, interruption;
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const result = (id, value) => send({ jsonrpc: '2.0', id, result: value });
const item = (kind, data, turn_id = null, push = true) => {
  const record = { session_id: 'due-session', cursor: ++cursor, turn_id, kind, data };
  journal.push(record);
  if (push) send({ jsonrpc: '2.0', method: 'session.item', params: { ...record, push_dropped: 0 } });
  return record;
};
const outcome = turn => ({ schema_version: 1, provider: 'codex', run_id: mode === 'wrong-run' ? 'stale-turn' : turn,
  session_id: mode === 'missing-id' ? null : mode.startsWith('init') ? 'claude-native' : 'native-one', status: mode === 'unknown-status' ? 'finished' : mode === 'failed' ? 'failed' : 'succeeded',
  text: `turn ${count}`, error: ['failed', 'false-success'].includes(mode) ? { kind: 'provider', message: 'synthetic failure' } : null });
const failedOutcome = turn => ({ ...outcome(turn), status: 'failed', error: { kind: 'protocol', message: 'native exited' } });
const cancelledOutcome = turn => ({ ...outcome(turn), status: 'cancelled', error: { kind: 'cancelled', message: 'interrupted' } });
const reject = (id, message) => send({ jsonrpc: '2.0', id, error: { code: -32000, message } });
const finishTurn = () => item('turn_finished', outcome(current), current);
const input = createInterface({ input: process.stdin });
input.on('line', line => {
  const request = JSON.parse(line);
  if (directory) appendFileSync(join(directory, 'fixture-input.jsonl'), line + '\n');
  const { method: name, params: args } = request;
  if (name === 'session.open') {
    directory = args.cwd; mode = args.model;
    appendFileSync(join(directory, 'fixture-input.jsonl'), line + '\n');
    item('session_opened', {});
    if (mode === 'open-failed') item('session_failed', { message: 'native exited during open' });
    return result(request.id, { session_id: 'due-session', native_session_id: mode.startsWith('init') ? null : 'native-one',
      protocol_version: mode === 'wrong-version' ? 9 : 1, stream: { enabled: true, method: 'session.item' } });
  }
  if (name === 'session.read') {
    const items = journal.filter(value => value.cursor > args.after).slice(0, 64);
    return result(request.id, { items, cursor: items.at(-1)?.cursor ?? args.after, items_lost: mode === 'lost', has_more: false });
  }
  if (name === 'turn.start') {
    if (mode === 'rpc-error') {
      send({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Fixture request rejected' } }); return;
    }
    current = `native-turn-${++count}`;
    item('turn_started', { prompt: args.prompt }, current);
    if (mode.startsWith('interrupt-')) {
      const turn = current;
      if (count > 1) {
        if (interruption && ['interrupt-late', 'interrupt-late-error'].includes(mode)) {
          if (mode === 'interrupt-late-error') reject(interruption.id, 'native rejected original interruption');
          else result(interruption.id, { interrupt_requested: true, turn_id: interruption.turn });
          interruption = null;
        }
        item('turn_finished', outcome(turn), turn);
        result(request.id, { turn_id: turn });
      } else if (mode === 'interrupt-finished-start') {
        item('turn_finished', outcome(turn), turn);
        setTimeout(() => result(request.id, { turn_id: turn }), 40);
      } else {
        // Delayed turn.start acknowledgement exercises controls bound from an earlier turn_started item.
        setTimeout(() => result(request.id, { turn_id: turn }), 40);
      }
      return;
    }
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
      if (mode === 'init-hold') { result(request.id, { turn_id: current }); return; }
    }
    if (mode === 'malformed') { process.stdout.write('not-json\n'); return; }
    if (mode === 'hold') { result(request.id, { turn_id: current }); return; }
    if (mode === 'wait-delayed') {
      result(request.id, { turn_id: current });
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
      if (mode === 'interaction-failed') { item('session_failed', { message: 'native exited awaiting permission' }); item('turn_finished', failedOutcome(current), current); }
      result(request.id, { turn_id: current }); return;
    }
    if (mode === 'invalid-utf8' || mode === 'valid-utf8') {
      // Raw-byte item. valid-utf8 splits a multi-byte value across writes and includes a real U+FFFD.
      const record = { session_id: 'due-session', cursor: ++cursor, turn_id: current, kind: 'native', data: { text: '\u0001' } };
      journal.push(record);
      const [head, rest] = JSON.stringify({ jsonrpc: '2.0', method: 'session.item', params: { ...record, push_dropped: 0 } }).split('\\u0001');
      const middle = Buffer.from(mode === 'valid-utf8' ? 'é€😀\uFFFD' : [0xff]);
      const cut = mode === 'valid-utf8' ? 3 : middle.length; // Mid-way through the euro sign.
      process.stdout.write(Buffer.concat([Buffer.from(head), middle.subarray(0, cut)]));
      setTimeout(() => { process.stdout.write(Buffer.concat([middle.subarray(cut), Buffer.from(rest + '\n')])); item('turn_finished', outcome(current), current); result(request.id, { turn_id: current }); }, 30);
      return;
    }
    if (mode === 'big-int') {
      // 2^53 + 1 is a valid JSON number that JavaScript's JSON.parse rounds to 2^53.
      const record = { session_id: 'due-session', cursor: ++cursor, turn_id: current, kind: 'native', data: { n: '\u0001' } };
      journal.push(record);
      const line = JSON.stringify({ jsonrpc: '2.0', method: 'session.item', params: { ...record, push_dropped: 0 } });
      process.stdout.write(line.replace('"\\u0001"', '9007199254740993') + '\n');
    }
    if (mode === 'flood') {
      // Ignored notifications padded with insignificant whitespace: small parsed view, 66 MiB of raw bytes.
      const frame = '{"jsonrpc":"2.0","method":"fixture/ignored"}' + ' '.repeat(1024 * 1024) + '\n';
      for (let i = 0; i < 66; i += 1) process.stdout.write(frame);
      return;
    }
    if (mode === 'active-failed') { item('session_failed', { message: 'native exited mid-turn' }); item('turn_finished', failedOutcome(current), current); return result(request.id, { turn_id: current }); }
    item('native', { provider: 'codex', method: 'item/completed', params: { item: { nested: { future: [null, 3, true] } } } }, current, !['gap', 'lost'].includes(mode));
    item('turn_finished', outcome(current), current);
    if (mode === 'idle-failed') setTimeout(() => item('session_failed', { message: 'native exited between turns' }), 60);
    // Native stream and terminal deliberately precede turn.start response.
    result(request.id, { turn_id: current });
  }
  if (name === 'turn.interrupt') {
    if (args.session_id !== 'due-session' || args.turn_id !== current) return reject(request.id, 'unknown or finished turn');
    if (mode === 'interrupt-hold') {
      interruption = { id: request.id, turn: current };
      item('native', { provider: 'codex', method: 'fixture/control-waiting', params: {} }, current);
      return;
    }
    if (['interrupt-late', 'interrupt-late-error', 'interrupt-on-close'].includes(mode)) {
      interruption = { id: request.id, turn: current };
      item('turn_finished', cancelledOutcome(current), current);
      return;
    }
    result(request.id, { interrupt_requested: true, turn_id: current });
    item('turn_finished', cancelledOutcome(current), current);
    return;
  }
  if (name === 'interaction.reply') {
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
    return result(request.id, { replied: true, request_id: 'permission' });
  }
  if (name === 'session.close' && held) {
    // The genuine reply failure arrives while close is outstanding; close completes only afterwards.
    reject(held, 'permissions backend unavailable'); held = null;
    setTimeout(() => { result(request.id, { closed: true }); item('session_closed', {}); }, 200);
    return;
  }
  if (name === 'session.close') {
    result(request.id, { closed: true });
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
