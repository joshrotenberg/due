// Client-side shape checks. Rust owns strict request-file parsing and policy validation.
import { realpath, stat } from 'node:fs/promises';
import { resolve } from 'node:path';

const shared = ['provider', 'cwd', 'model', 'effort', 'timeout'];
const rpc = new Set([...shared, 'request', 'binary', 'resume', 'ephemeral', 'native_args']);
const own = (value, key) => Object.hasOwn(value, key);

export async function sessionProfile(input, backend) {
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('profile must be an object');
  const allowed = backend === 'rpc' ? rpc : new Set(shared);
  for (const key of Object.keys(input)) {
    if (!allowed.has(key)) throw new Error(`unsupported shared profile field: ${key}; use request for an existing JSON profile with rpc`);
  }
  const profile = { ...input };
  // JavaScript undefined is omitted on the wire; preserve that inheritance here.
  for (const key of Object.keys(profile)) if (profile[key] === undefined) delete profile[key];
  if (own(profile, 'request')) {
    if (typeof profile.request !== 'string' || !profile.request.trim() || profile.request === '-') {
      throw new Error('request must name a JSON profile file; stdin is reserved for RPC');
    }
    profile.request = resolve(profile.request);
  }
  if ((own(profile, 'provider') || !own(profile, 'request')) && !['claude', 'codex'].includes(profile.provider)) {
    throw new Error('provider must be claude or codex, or supplied by a request file');
  }
  for (const key of ['cwd', 'model', 'effort', 'timeout', 'binary', 'resume']) {
    if (!own(profile, key) || profile[key] === undefined) continue;
    if (backend === 'rpc' && profile[key] === null) continue;
    if (typeof profile[key] !== 'string' || !profile[key].trim()) throw new Error(`${key} must be a nonempty string`);
  }
  if (profile.effort != null && !['low', 'medium', 'high'].includes(profile.effort)) throw new Error('effort must be low, medium or high');
  if (own(profile, 'ephemeral') && typeof profile.ephemeral !== 'boolean') throw new Error('ephemeral must be boolean');
  if (own(profile, 'native_args')) {
    if (!Array.isArray(profile.native_args) || !profile.native_args.every(value => typeof value === 'string')) {
      throw new Error('native_args must be an array of strings');
    }
    profile.native_args = Object.freeze([...profile.native_args]);
  }
  if (typeof profile.cwd === 'string' || (!own(profile, 'request') && profile.cwd !== null)) {
    profile.cwd = await realpath(profile.cwd ?? '.');
    if (!(await stat(profile.cwd)).isDirectory()) throw new Error('cwd must be a directory');
  }
  // Omission inherits the file value; null is an explicit override for RPC.
  return Object.freeze(profile);
}
