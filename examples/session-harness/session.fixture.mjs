#!/usr/bin/env node
// Synthetic protocol peer for the shared correction example. No native model.
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';

const output = value => process.stdout.write(`${JSON.stringify(value)}\n`);
function answer(prompt, state) {
  if (prompt.startsWith('Correction:')) {
    if (state.marker !== 'larch-4821') throw new Error('Missing conversation context');
    state.shipping = 3;
    return { marker: state.marker, revision: 2, total: state.count * state.price + state.shipping };
  }
  if (!prompt.includes('larch-4821')) throw new Error('Unexpected test prompt');
  Object.assign(state, { marker: 'larch-4821', count: 2, price: 7, shipping: 5 });
  return { marker: state.marker, revision: 1, total: 19 };
}
const result = (provider, nativeId, text) => ({ schema_version: 1, provider, status: 'succeeded',
  session_id: nativeId, run_id: randomUUID(), text: JSON.stringify(text), error: null });

if (process.argv[2] === 'run') {
  let input = ''; for await (const part of process.stdin) input += part;
  const request = JSON.parse(input);
  const nativeId = request.resume ?? randomUUID();
  const file = join(request.cwd, `memory-${nativeId}.json`);
  const state = request.resume ? JSON.parse(readFileSync(file, 'utf8')) : {};
  const value = answer(request.prompt, state);
  writeFileSync(file, JSON.stringify(state));
  appendFileSync(join(request.cwd, 'launches.jsonl'), `${JSON.stringify({ type: 'run', pid: process.pid, resume: request.resume ?? null, nativeId })}\n`);
  process.stderr.write(`${JSON.stringify({ schema_version: 1, run_id: `due-${process.pid}`, event: 'session_started', session_id: nativeId })}\n`);
  output(result(request.provider, nativeId, value));
} else if (process.argv[2] === 'mcp') {
  let provider, nativeId, dueId, cwd, state = {}, cursor = 0, items = [];
  const respond = (id, value) => output({ jsonrpc: '2.0', id, result: value });
  const tool = (id, value) => respond(id, { structuredContent: value, content: [] });
  function item(kind, data, turn_id = null) {
    const params = { session_id: dueId, turn_id, cursor: ++cursor, kind, data };
    items.push(params);
    output({ jsonrpc: '2.0', method: 'notifications/due/session_item', params: { ...params, push_dropped: 0 } });
  }
  for await (const line of createInterface({ input: process.stdin })) {
    const message = JSON.parse(line);
    if (message.method === 'initialize') { respond(message.id, { protocolVersion: '2025-11-25' }); continue; }
    if (message.method === 'notifications/initialized') continue;
    const { name, arguments: args } = message.params;
    if (name === 'session_open') {
      ({ provider, cwd } = args); nativeId = randomUUID(); dueId = randomUUID();
      appendFileSync(join(cwd, 'launches.jsonl'), `${JSON.stringify({ type: `${process.argv[2]}-open`, pid: process.pid, nativeId })}\n`);
      item('session_opened', { provider });
      tool(message.id, { session_id: dueId, native_session_id: nativeId });
    } else if (name === 'turn_start') {
      const id = randomUUID();
      item('turn_started', { prompt: args.prompt }, id);
      item('native', { provider, method: 'item/completed', params: { item: { unknown: { nested: [null, 7, { keep: true }] } } } }, id);
      item('turn_finished', { ...result(provider, nativeId, answer(args.prompt, state)), run_id: id }, id);
      tool(message.id, { turn_id: id, session_id: dueId });
    } else if (name === 'session_read') {
      tool(message.id, { items: items.filter(item => item.cursor > args.after), cursor, latest_cursor: cursor, has_more: false, items_lost: false });
    } else if (name === 'session_close') {
      item('session_closed', {}); tool(message.id, { closed: true, native_session_id: nativeId });
    } else throw new Error(`Unexpected tool: ${name}`);
  }
} else throw new Error('Unexpected Due command');
