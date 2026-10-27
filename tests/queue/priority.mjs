// Real submission, priority mutation and cancellation against an isolated local
// store. No scheduler or provider is simulated; the queue's persisted documents
// and index keys are the observed result. No worker is launched.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = realpathSync(required('STADO_BIN'));
const negative = Number(required('STADO_TEST_PRIORITY_NEGATIVE'));
const high = Number.MAX_SAFE_INTEGER;
assert.ok(Number.isSafeInteger(negative) && negative < high);
const home = join(repository, '.build', 'queue-priority', randomUUID());
const store = join(home, 'store');
mkdirSync(store, { recursive: true });
mkdirSync(join(home, 'tmp'));
const environment = {
  PATH: required('PATH'), HOME: home, TMPDIR: join(home, 'tmp'),
  STADO_CONFIG: join(home, '.stado', 'config.json'),
  WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: store,
};
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const report = {
  source_revision: required('STADO_TEST_SOURCE_REVISION'),
  binary, binary_sha256: sha(readFileSync(binary)),
  journey_sha256: sha(readFileSync(fileURLToPath(import.meta.url))),
  started_at: new Date().toISOString(), commands: [], verdict: 'failed',
};
const save = () => writeFileSync(join(home, 'report.json'), JSON.stringify(report, null, '  '));
function command(args, succeeds = true) {
  const result = spawnSync(binary, args, { cwd: repository, env: environment, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr,
    error: result.error?.message });
  save();
  assert.ifError(result.error);
  assert.equal(exitedCleanly(result), succeeds, result.stderr);
  return result;
}
function records(prefix) {
  const directory = join(store, prefix);
  let names;
  try { names = readdirSync(directory); }
  catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
  return names.filter(name => !name.startsWith('.') && name.endsWith('.json')).sort()
    .map(name => ({ name, value: JSON.parse(readFileSync(join(directory, name), 'utf8')) }));
}
function queued() { return records('queue').map(row => row.value); }
function order() { return records('queue_priority').map(row => row.value.job_id); }
function persisted(label) {
  const job = queued().find(value => value.run_id === label);
  assert.ok(job, `submission ${label} did not persist a queued job`);
  return job;
}
function submit(label, priority) {
  command(['submit', '--run-id', label, `--priority=${priority}`, 'true']);
  const job = persisted(label);
  assert.equal(job.priority, priority);
  return job.job_id;
}
const submitted = new Set();
try {
  command(['config', 'init']);
  // A submission naming no priority takes the one `stado submit` declares by
  // default, the lowest it admits; the journey reads it from the job the
  // product persisted instead of being told it.
  command(['submit', '--run-id', 'priority-lower', 'true']);
  const lowest = persisted('priority-lower');
  const base = lowest.priority;
  assert.ok(Number.isSafeInteger(base) && base < high, `default priority ${base}`);
  assert.ok(negative < base, `STADO_TEST_PRIORITY_NEGATIVE ${negative} must be below the default ${base}`);
  report.default_priority = base;
  const lower = lowest.job_id;
  submitted.add(lower);
  const higher = submit('priority-higher', high);
  submitted.add(higher);
  assert.deepEqual(order(), [higher, lower], 'the integer key must order a priority beyond the old decimal range');
  command(['job', 'prioritize', lower, String(high), '--json']);
  assert.equal(queued().find(job => job.job_id === lower).priority, high);
  assert.deepEqual(order(), [lower, higher], 'equal priorities must retain creation order');
  command(['job', 'prioritize', higher, String(base), '--json']);
  assert.equal(queued().find(job => job.job_id === higher).priority, base);
  assert.deepEqual(order(), [lower, higher], 'demotion must remove the superseded index entry');
  const before = queued();
  const refusedSubmit = command(['submit', '--run-id', 'priority-refused', `--priority=${negative}`, 'true'], false);
  assert.ok(refusedSubmit.stderr.includes('must be nonnegative'), refusedSubmit.stderr);
  assert.deepEqual(queued(), before, 'refused submission changed the queue');
  const refusedChange = command(['job', 'prioritize', lower, '--json', '--', String(negative)], false);
  assert.ok(refusedChange.stderr.includes('must be nonnegative'), refusedChange.stderr);
  assert.deepEqual(queued(), before, 'refused priority mutation changed persisted jobs');
  report.queue_before_cancel = before;
  report.markers_before_cancel = records('queue_priority');
  for (const id of submitted) command(['cancel', id]);
  assert.deepEqual(queued(), [], 'cancelled jobs remained queued');
  assert.deepEqual(order(), [], 'cancelled jobs retained claimable markers');
  const refusedTerminal = command(['job', 'prioritize', lower, String(high), '--json'], false);
  assert.ok(refusedTerminal.stderr.includes('not queued'), refusedTerminal.stderr);
  assert.deepEqual(queued(), [], 'a terminal job was requeued by a refused mutation');
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack;
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  report.final_queue = queued();
  report.final_markers = records('queue_priority');
  save();
  console.log(`${report.verdict}: ${join(home, 'report.json')}`);
}
