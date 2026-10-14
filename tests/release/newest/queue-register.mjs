// Real journey of `stado release newest --register`, the read the fleet
// report takes its release figures from, through the built Stado. The
// queue's register is a file of this run (STADO_RELEASE_LOOP_REGISTER), so
// the operator's queue is never read or written.
//
// 1. A register the queue has not written yet answers no entries, naming
//    the path it read.
// 2. A register holding a delivered commit (with the time its release was
//    published and the time Oko recorded it) and a commit the release step
//    refused (with the step's error) answers both, newest first, every field
//    kept: the report's success rate and times are read from exactly these.
// 3. A register that is not one is refused naming its path; it is never
//    read as an empty queue.
//
// Needs STADO_BIN and STADO_TEST_SUCCESS_EXIT.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const stadoBinary = realpathSync(required('STADO_BIN'));
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const output = join(root, '.build', 'release-queue-register', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const register = join(output, 'release-loop.json');
const environment = { ...process.env, STADO_RELEASE_LOOP_REGISTER: register };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const revision = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim();
const report = {
  started_at: new Date().toISOString(), revision,
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  commands: [], verdict: 'failed',
};
function read(json) {
  const args = ['release', 'newest', '--register', ...(json ? ['--json'] : [])];
  const result = spawnSync(stadoBinary, args, { cwd: output, env: environment, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}

const delivered = {
  product: 'register-delivered', commit: revision, state: 'delivered', step: 'record', version: '1.2.3',
  carried: ['DEFECT-DELIVERED'], recorded: ['DEFECT-DELIVERED'],
  started_at: '2026-10-14T01:00:00Z', updated_at: '2026-10-14T01:30:00Z',
  released_at: '2026-10-14T01:20:00Z', recorded_at: '2026-10-14T01:30:00Z',
};
const refused = {
  product: 'register-refused', commit: revision, state: 'failed', step: 'release', version: '4.5.6',
  carried: ['DEFECT-REFUSED'], recorded: [], error: 'the build of register-refused failed: tests/quality refused',
  started_at: '2026-10-14T02:00:00Z', updated_at: '2026-10-14T02:10:00Z',
};

try {
  const empty = read(true);
  assert.equal(empty.status, successExit, empty.stderr);
  const none = JSON.parse(empty.stdout);
  assert.equal(none.path, register);
  assert.deepEqual(none.entries, []);

  writeFileSync(register, JSON.stringify({ entries: [delivered, refused] }));
  const kept = read(true);
  assert.equal(kept.status, successExit, kept.stderr);
  const entries = JSON.parse(kept.stdout).entries;
  assert.deepEqual(entries.map(entry => entry.product), [refused.product, delivered.product], 'not newest first');
  const [failed, done] = entries;
  assert.equal(failed.state, 'failed');
  assert.equal(failed.step, 'release');
  assert.equal(failed.error, refused.error);
  assert.equal(failed.released_at, undefined, 'a refused release was given a publication time');
  assert.equal(new Date(done.released_at).toISOString(), new Date(delivered.released_at).toISOString());
  assert.equal(new Date(done.recorded_at).toISOString(), new Date(delivered.recorded_at).toISOString());
  assert.deepEqual(done.recorded, delivered.recorded);
  const lines = read(false);
  assert.equal(lines.status, successExit, lines.stderr);
  assert.ok(lines.stdout.includes(`register ${register}`), lines.stdout);
  assert.ok(lines.stdout.includes(refused.product) && lines.stdout.includes(delivered.product), lines.stdout);

  writeFileSync(register, 'not a register');
  const broken = read(true);
  assert.notEqual(broken.status, successExit, 'a register that is not one was read');
  assert.ok(broken.stderr.includes(`the delivery loop register ${register} is not a register`), broken.stderr);
  report.verdict = 'passed';
} finally {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
