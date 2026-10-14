// Submit real jobs into an isolated local store, then inspect its persisted objects
// through the CLI. No scheduler, provider, or object-store response is simulated.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { availableParallelism } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { snapshotSubject, verifyRevision } from '../../native/subject.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const count = Number(required('STADO_TEST_LIST_COUNT'));
const limitInput = required('STADO_TEST_LIST_LIMIT');
assert.match(limitInput, /^\d+$/, 'STADO_TEST_LIST_LIMIT must be an unsigned integer');
const limit = Number(limitInput);
assert.ok(Number.isSafeInteger(count) && Number.isSafeInteger(limit) && count > limit,
  'STADO_TEST_LIST_COUNT must exceed STADO_TEST_LIST_LIMIT');
const parallelism = availableParallelism();
assert.ok(count > parallelism, 'STADO_TEST_LIST_COUNT must exceed the measured host parallelism');
const revision = required('STADO_TEST_SOURCE_REVISION');
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
const explicitConcurrency = required('STADO_TEST_COPY_CONCURRENCY');
const refusedConcurrency = required('STADO_TEST_COPY_REFUSED_CONCURRENCY');
const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const output = join(repository, '.build', 'storage-listing', randomUUID());
const home = join(output, 'home');
const store = join(home, 'store');
mkdirSync(store, { recursive: true });
mkdirSync(join(home, 'tmp'));
const environment = {
  PATH: required('PATH'), HOME: home, TMPDIR: join(home, 'tmp'),
  STADO_CONFIG: join(home, '.stado', 'config.json'),
  WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: store,
};
const report = { source_revision: revision, count, limit, parallelism,
  started_at: new Date().toISOString(), commands: [], verdict: 'failed' };
const save = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
let subject;
function command(args, succeeds = true) {
  const result = spawnSync(subject.path, args, { cwd: repository, env: environment, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  save();
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'a terminated process is not a product refusal');
  assert.equal(result.status === successExit, succeeds, result.stderr);
  return result;
}
function snapshot(root = store) {
  return readdirSync(join(root, 'queue')).filter(name => name.endsWith('.json')).sort()
    .map(name => ({ name: `queue/${name}`, sha256: digest(readFileSync(join(root, 'queue', name))) }));
}
const names = result => result.objects.map(object => object.name);
try {
  subject = snapshotSubject(required('STADO_BIN'), output);
  report.subject = subject;
  report.journey_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.binary_version = command(['--version']).stdout.trim();
  verifyRevision(report.binary_version, revision);
  command(['config', 'init']);
  for (const label of Array.from({ length: count }, () => randomUUID())) {
    command(['submit', '--run-id', label, 'true']);
  }
  const before = snapshot();
  assert.equal(before.length, count, 'every submission must persist a queued object');
  report.before = before;
  const expected = before.map(object => object.name);
  const statuses = JSON.parse(command(['status', '--json']).stdout);
  const persistedJobs = expected.map(name => JSON.parse(readFileSync(join(store, name), 'utf8')));
  const byId = (left, right) => left.job_id.localeCompare(right.job_id);
  assert.deepEqual(
    statuses.sort(byId),
    persistedJobs.map(job => ({ ...job, state: 'queue', reaped: false })).sort(byId),
    'status must return every persisted job across measured read batches without changing its fields',
  );
  const prefix = expected.filter((_, index) => index < limit);
  const listing = (...args) => JSON.parse(command(['storage', 'ls', 'queue/', '--json', ...args]).stdout);
  const complete = listing();
  assert.deepEqual(names(complete), expected, 'omitted limit must return every persisted object');
  assert.equal(complete.limit, null);
  assert.equal(complete.truncated, false);
  assert.equal(complete.listed, count);
  assert.equal(complete.total, count);
  const bounded = listing('--limit', String(limit));
  assert.deepEqual(names(bounded), prefix);
  assert.equal(bounded.limit, limit);
  assert.equal(bounded.truncated, true);
  assert.equal(bounded.total, count);
  const exact = listing('--limit', String(count));
  assert.deepEqual(names(exact), expected);
  assert.equal(exact.truncated, false);
  const sized = listing('--size', '--limit', String(limit));
  assert.deepEqual(names(sized), prefix);
  for (const object of sized.objects) {
    assert.equal(object.size, readFileSync(join(store, object.name)).byteLength);
    assert.equal(object.size_error, null);
  }
  const text = command(['storage', 'ls', 'queue/']).stdout;
  for (const name of expected) assert.ok(text.includes(name), `text output omitted ${name}`);
  const refused = command(['storage', 'ls', 'queue/', '--limit', 'not-an-integer'], false);
  assert.ok(refused.stderr.includes('--limit'), refused.stderr);
  report.copies = [];
  for (const selection of [
    { name: 'measured', flags: [] },
    { name: 'explicit', flags: ['--concurrency', explicitConcurrency] },
  ]) {
    const destination = join(home, selection.name);
    const endpoints = ['--from', 'local', '--from-path', store,
      '--to', 'local', '--to-path', destination, '--prefix', 'queue/'];
    command(['storage', 'copy', ...endpoints, ...selection.flags]);
    const copied = snapshot(destination);
    assert.deepEqual(copied, before, `${selection.name} copy must preserve every object's bytes`);
    command(['storage', 'verify', ...endpoints, '--json']);
    report.copies.push({ selection: selection.name, destination, objects: copied });
  }
  const refusedDestination = join(home, 'refused-copy');
  const copyRefusal = command(['storage', 'copy', '--from', 'local', '--from-path', store,
    '--to', 'local', '--to-path', refusedDestination, '--prefix', 'queue/',
    '--concurrency', refusedConcurrency], false);
  assert.ok(copyRefusal.stderr.includes('--concurrency'), copyRefusal.stderr);
  assert.equal(existsSync(refusedDestination), false, 'invalid fan-out must be refused before opening the destination');
  report.after = snapshot();
  assert.deepEqual(report.after, before, 'inspection and argument refusals must not mutate stored objects');
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack;
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  save();
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
