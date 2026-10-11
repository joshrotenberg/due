#!/usr/bin/env node
// Task-specific checks only: this is not a general JSON Schema validator.
import { closeSync, openSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const [dir, id, expectedText, ...extra] = process.argv.slice(2);
const expectedPr = Number(expectedText);
const validId = typeof id === 'string' && id.length <= 80 &&
  /^[A-Za-z0-9]/.test(id) && !/[^A-Za-z0-9_-]/.test(id);
if (!dir || !validId || !expectedText || /\D/.test(expectedText) || !Number.isSafeInteger(expectedPr) ||
    expectedPr <= 0 || extra.length) {
  console.error('Usage: node validate-review.mjs DIR ID EXPECTED_PR\nID must be 1–80 ASCII letters/digits/_/-, starting with a letter or digit; EXPECTED_PR must be a positive safe integer.');
  process.exit(2);
}

function require(condition, message) {
  if (!condition) throw new Error(message);
}
function object(value, label) {
  require(value !== null && typeof value === 'object' && !Array.isArray(value), `${label} must be an object`);
}
function exactKeys(value, keys, label) {
  object(value, label);
  require(Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key)),
    `${label} must contain exactly these keys: ${keys.join(', ')}`);
}
function string(value, label) {
  require(typeof value === 'string', `${label} must be a string`);
}
function nullableId(value, label) {
  require(value === undefined || value === null || typeof value === 'string', `${label} must be a string or null when present`);
  return value ?? null;
}
function readJson(path) {
  try { return JSON.parse(readFileSync(path, 'utf8')); }
  catch (error) { throw new Error(`Cannot read JSON from ${JSON.stringify(path)}: ${error.message}. Check DIR/ID and the saved run files.`); }
}
function writeExclusive(path, value) {
  let fd;
  try {
    fd = openSync(path, 'wx', 0o600);
    writeFileSync(fd, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
    closeSync(fd);
    fd = undefined;
  } catch (error) {
    // Remove only an incomplete file created by this invocation, never an existing one.
    if (fd !== undefined) {
      try { closeSync(fd); } catch {}
      try { unlinkSync(path); } catch {}
    }
    throw new Error(`Cannot create ${JSON.stringify(path)} exclusively: ${error.message}. Use a fresh run ID; existing artifacts are never overwritten.`);
  }
}

try {
  const runDir = join(dir, `${id}.run`);
  const exit = readJson(join(runDir, 'exit.json'));
  object(exit, 'exit.json');
  require(exit.exit_code === 0, 'Due must exit with exit_code 0 before accepting this review');
  for (const key of ['signal', 'launch_error', 'stdin_error', 'signal_received']) {
    require(exit[key] === null, `exit.json ${key} must be null before accepting this review`);
  }
  const result = readJson(join(runDir, 'result.json'));
  object(result, 'result.json');
  require(result.schema_version === 1, 'Due result schema_version must be 1');
  require(result.status === 'succeeded', 'Due result status must be succeeded');
  require(result.error === null, 'Due result error must be null');
  require(result.structured_output_present === true, 'Due must report structured_output_present true');
  const runId = nullableId(result.run_id, 'Due run_id');
  const sessionId = nullableId(result.session_id, 'Due session_id');
  // This is a delivery requirement for this example, not proof of answer correctness.
  object(result.trace, 'Due trace delivery metadata');
  require(result.trace.incomplete === false && result.trace.dropped_records === 0,
    'This example requires complete trace delivery: incomplete false and dropped_records 0');
  const answer = result.structured_output;
  exactKeys(answer, ['pr', 'verdict', 'summary', 'findings', 'limitations'], 'review answer');
  require(Number.isSafeInteger(answer.pr) && answer.pr > 0 && answer.pr === expectedPr,
    `review answer pr must be the expected positive safe integer ${expectedPr}`);
  require(['approve', 'request_changes'].includes(answer.verdict), 'review verdict must be approve or request_changes');
  string(answer.summary, 'review summary');
  require(Array.isArray(answer.limitations), 'review limitations must be an array');
  answer.limitations.forEach((value, i) => string(value, `review limitations[${i}]`));
  require(Array.isArray(answer.findings), 'review findings must be an array');
  answer.findings.forEach((finding, i) => {
    const label = `review findings[${i}]`;
    exactKeys(finding, ['priority', 'file', 'line', 'description', 'validation'], label);
    require(['P1', 'P2', 'P3'].includes(finding.priority), `${label} priority must be P1, P2 or P3`);
    require(Number.isSafeInteger(finding.line) && finding.line > 0, `${label} line must be a positive safe integer`);
    for (const key of ['file', 'description', 'validation']) string(finding[key], `${label} ${key}`);
  });
  const validated = { id, valid: true, run_id: runId, session_id: sessionId,
    status: result.status, verdict: answer.verdict, structured_answer_validated_independently: true };
  writeExclusive(join(runDir, 'validated.json'), validated);
  console.log(JSON.stringify(validated));
} catch (error) {
  console.error(`Review validation failed: ${error.message}`);
  process.exitCode = 1;
}
