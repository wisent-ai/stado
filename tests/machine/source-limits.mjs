// Real CLI source staging, persisted upload, replay and refusal qualification.
// STADO_BIN names the candidate. No worker or external store is started.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

assert.ok(process.env.STADO_BIN, 'STADO_BIN must name the candidate executable');
const binary = realpathSync(process.env.STADO_BIN);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const parent = join(root, 'build', 'machine-source-limits');
mkdirSync(parent, { recursive: true });
const output = mkdtempSync(join(parent, 'run-'));
const home = join(output, 'home');
const store = join(home, 'store');
const source = join(output, 'source');
const config = join(home, 'config.json');
const archive = join(output, 'source.tar.gz');
mkdirSync(home);
mkdirSync(source);
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config, WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: store };
const report = { started_at: new Date().toISOString(), verdict: 'failed', commands: [], cases: [] };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function command(program, args, overrides = {}) {
  const result = spawnSync(program, args, { cwd: root, env: { ...environment, ...overrides }, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr, ...overrides });
  if (result.error) throw result.error;
  assert.ok(Number.isInteger(result.status), `command did not exit normally: ${program}`);
  return result;
}
function success(args) {
  const result = command(binary, args);
  assert.ok(!result.status, `${args.join(' ')}: ${result.stderr}\n${result.stdout}`);
  return result.stdout;
}
function request(name, archivePath = archive) {
  const id = `source-limits-${name}-${randomUUID()}`;
  const path = join(output, `${id}.json`);
  const value = { client_request_id: id, command: '/usr/bin/true', provider: 'local', source_archive_path: archivePath };
  writeFileSync(path, JSON.stringify(value));
  return { id, path, record: join(store, 'machine_requests', `${id}.json`) };
}
function submit(input, overrides = {}) {
  const result = command(binary, ['machine', 'submit', '--request-file', input.path], overrides);
  assert.ok(result.stdout.trim(), `machine submit returned no JSON (exit ${result.status}): ${result.stderr}`);
  return { result, receipt: JSON.parse(result.stdout) };
}
function refuse(name, input, overrides, field) {
  const before = existsSync(input.record) ? readFileSync(input.record, 'utf8') : null;
  const { result, receipt } = submit(input, overrides);
  const after = existsSync(input.record) ? readFileSync(input.record, 'utf8') : null;
  const passed = Boolean(result.status) && receipt.ok === false && receipt.error?.code === 'INVALID_SOURCE_ARCHIVE'
    && receipt.error.message.includes(field) && before === after;
  report.cases.push({ name, receipt, record_before: before, record_after: after, verdict: passed ? 'passed' : 'failed' });
  if (receipt.ok === true) success(['machine', 'cancel', receipt.result.job.job_id]);
}
try {
  report.source_revision = command('git', ['rev-parse', 'HEAD']).stdout.trim();
  report.source_patch = command('git', ['diff', '--binary']).stdout;
  report.binary = binary;
  report.binary_sha256 = digest(readFileSync(binary));
  report.binary_version = success(['--version']).trim();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const files = new Map([['alpha.txt', 'source alpha\n'], ['beta.txt', 'source beta\n']]);
  for (const [name, body] of files) writeFileSync(join(source, name), body);
  success(['storage', 'archive', source, archive, '--json']);
  const archiveContent = readFileSync(archive);
  const members = readdirSync(source);
  const smallerMemberSet = [...members];
  smallerMemberSet.pop();
  const memberSizes = members.map(name => statSync(join(source, name)).size);
  const smallerByteBudget = Math.min(...memberSizes);
  const limits = {
    archive_bytes: archiveContent.byteLength,
    extracted_bytes: memberSizes.reduce((total, bytes) => total + bytes),
    members: members.length,
    trailing_bytes: gunzipSync(archiveContent).byteLength,
  };
  assert.ok(limits.archive_bytes > smallerByteBudget && limits.extracted_bytes > smallerByteBudget);
  report.archive = { path: archive, sha256: digest(archiveContent), limits, files: Object.fromEntries(files) };
  refuse('missing-declaration', request('missing'), {}, 'machine.source_limits');
  success(['config', 'set', 'machine.source_limits', JSON.stringify(limits)]);
  const invalidPolicies = [
    { name: 'invalid-json', value: '{', field: 'WC_MACHINE_SOURCE_LIMITS' },
    { name: 'missing-bound', value: JSON.stringify({ archive_bytes: limits.archive_bytes }), field: 'machine.source_limits' },
    { name: 'invalid-bound', value: JSON.stringify({ ...limits, members: false }), field: 'machine.source_limits' },
    { name: 'unknown-bound', value: JSON.stringify({ ...limits, unused: true }), field: 'machine.source_limits' },
    { name: 'archive-byte-bound', value: JSON.stringify({ ...limits, archive_bytes: smallerByteBudget }), field: 'archive_bytes' },
    { name: 'extracted-byte-bound', value: JSON.stringify({ ...limits, extracted_bytes: smallerByteBudget }), field: 'extracted_bytes' },
    { name: 'member-bound', value: JSON.stringify({ ...limits, members: smallerMemberSet.length }), field: 'members' },
    { name: 'trailing-byte-bound', value: JSON.stringify({ ...limits, trailing_bytes: smallerByteBudget }), field: 'trailing_bytes' },
  ];
  for (const entry of invalidPolicies) {
    refuse(entry.name, request(entry.name), { WC_MACHINE_SOURCE_LIMITS: entry.value }, entry.field);
  }
  const link = join(output, 'linked.tar.gz');
  symlinkSync(archive, link);
  refuse('symlink-source', request('symlink', link), {}, 'non-symlink');
  const accepted = request('accepted');
  const first = submit(accepted);
  assert.ok(!first.result.status && first.receipt.ok === true, JSON.stringify(first.receipt));
  const receipt = first.receipt.result;
  const job = receipt.job.job_id;
  const expectedHash = digest(archiveContent);
  assert.equal(receipt.source_sha256, expectedHash);
  assert.equal(receipt.source_archive_uri, `stado://machine-inputs/${accepted.id}/${expectedHash}.tar.gz`);
  const stored = readFileSync(join(store, 'ecosystem', 'machine-inputs', accepted.id, `${expectedHash}.tar.gz`));
  assert.deepEqual(stored, archiveContent);
  const reservation = JSON.parse(readFileSync(accepted.record, 'utf8'));
  assert.equal(reservation.state, 'accepted');
  assert.equal(reservation.source_size_bytes, limits.archive_bytes);
  const status = JSON.parse(success(['machine', 'status', job]));
  assert.equal(status.result.job.job_id, job);
  const replay = submit(accepted);
  assert.ok(!replay.result.status && replay.receipt.ok === true, JSON.stringify(replay.receipt));
  assert.equal(replay.receipt.result.job.job_id, job);
  assert.equal(replay.receipt.result.source_sha256, expectedHash);
  report.cases.push({ name: 'exact-bounds-persist-and-replay', receipt, reservation, status, replay: replay.receipt, verdict: 'passed' });
  success(['config', 'set', 'machine.source_limits', JSON.stringify({ ...limits, archive_bytes: smallerByteBudget })]);
  refuse('retained-source-respects-current-bound', accepted, {}, 'archive_bytes');
  const overridden = submit(accepted, { WC_MACHINE_SOURCE_LIMITS: JSON.stringify(limits) });
  assert.ok(!overridden.result.status && overridden.receipt.ok === true, JSON.stringify(overridden.receipt));
  assert.equal(overridden.receipt.result.job.job_id, job);
  assert.deepEqual(JSON.parse(readFileSync(accepted.record, 'utf8')), reservation);
  report.cases.push({ name: 'environment-policy-overrides-stored-policy', receipt: overridden.receipt, verdict: 'passed' });
  const cancellation = JSON.parse(success(['machine', 'cancel', job]));
  const cancelled = JSON.parse(success(['machine', 'status', job]));
  assert.equal(cancelled.result.job.state, 'cancelled');
  report.cases.push({ name: 'isolated-job-cancelled', cancellation, status: cancelled, verdict: 'passed' });
  const work = join(home, '.stado', 'work', 'stado', 'machine-sources');
  if (existsSync(work)) assert.deepEqual(readdirSync(work), [], 'staged source files survived their request');
  assert.ok(report.cases.every(entry => entry.verdict === 'passed'), JSON.stringify(report.cases));
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack ?? error);
  throw error;
} finally {
  rmSync(home, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
