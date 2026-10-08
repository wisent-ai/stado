// Exercise real archive creation, extraction, deterministic bytes and safe refusals.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

assert.ok(process.env.STADO_BIN, 'STADO_BIN must name the candidate executable');
assert.ok(process.env.STADO_TEST_SUCCESS_EXIT, 'STADO_TEST_SUCCESS_EXIT must declare the successful process exit status');
const successExit = Number(process.env.STADO_TEST_SUCCESS_EXIT);
assert.ok(Number.isInteger(successExit));
const binary = realpathSync(process.env.STADO_BIN);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = join(root, 'build', 'storage-archive');
mkdirSync(build, { recursive: true });
const output = mkdtempSync(join(build, 'run-'));
const home = join(output, 'home');
const source = join(output, 'source');
const extracted = join(output, 'extracted');
for (const directory of [home, source, extracted]) mkdirSync(directory);
const config = join(home, 'config.json');
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { started_at: new Date().toISOString(), commands: [], cases: [], verdict: 'failed' };
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
function command(program, args, overrides = {}) {
  const result = spawnSync(program, args, { cwd: root, env: { ...environment, ...overrides }, encoding: 'utf8' });
  report.commands.push({ program, args, overrides, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  if (result.error) throw result.error;
  assert.ok(Number.isInteger(result.status), `${program} did not exit normally`);
  return result;
}
function success(program, args, overrides = {}) {
  const result = command(program, args, overrides);
  assert.equal(result.status, successExit, `${result.stderr}\n${result.stdout}`);
  return result.stdout;
}
function refused(name, overrides, reason, sourcePath = source) {
  const path = join(output, `${name}.tar.gz`);
  const result = command(binary, ['storage', 'archive', sourcePath, path, '--json'], overrides);
  const outputExists = existsSync(path);
  const passed = result.status !== successExit && !outputExists && result.stderr.includes(reason);
  report.cases.push({ name, status: result.status, output_exists: outputExists,
    diagnostic: result.stderr, verdict: passed ? 'passed' : 'failed' });
}
try {
  report.source_revision = success('git', ['rev-parse', 'HEAD']).trim();
  report.source_patch = success('git', ['diff', '--binary', 'HEAD']);
  report.binary = { path: binary, sha256: hash(readFileSync(binary)), version: success(binary, ['--version']).trim() };
  report.test_sha256 = hash(readFileSync(fileURLToPath(import.meta.url)));
  const files = new Map([['alpha.txt', 'alpha payload\n'], ['żółw.txt', 'beta payload\n']]);
  for (const [name, body] of files) writeFileSync(join(source, name), body);
  const names = [...files.keys()];
  const sizes = [...files.values()].map(body => Buffer.byteLength(body));
  const nameSizes = names.map(name => Buffer.byteLength(name));
  const fewerNames = [...names];
  fewerNames.pop();
  const limits = {
    entries: files.size,
    path_bytes: Math.max(...nameSizes),
    member_bytes: Math.max(...sizes),
    total_bytes: sizes.reduce((total, bytes) => total + bytes),
  };
  report.fixture = { files: Object.fromEntries(files), limits };
  refused('missing-declaration', {}, 'storage.archive_limits');
  success(binary, ['config', 'init']);
  success(binary, ['config', 'set', 'storage.archive_limits', JSON.stringify(limits)]);
  assert.deepEqual(JSON.parse(readFileSync(config, 'utf8')).storage.archive_limits, limits);
  const archive = join(output, 'accepted.tar.gz');
  const receipt = JSON.parse(success(binary, ['storage', 'archive', source, archive, '--json']));
  const bytes = readFileSync(archive);
  assert.equal(receipt.sha256, hash(bytes));
  assert.equal(receipt.bytes, bytes.byteLength);
  success('tar', ['-xzf', archive, '-C', extracted]);
  for (const [name, body] of files) assert.equal(readFileSync(join(extracted, name), 'utf8'), body);
  const repeated = join(output, 'repeated.tar.gz');
  success(binary, ['storage', 'archive', source, repeated, '--json']);
  assert.deepEqual(readFileSync(repeated), bytes);
  report.cases.push({ name: 'exact-bounds-extract-and-repeat', receipt, files: Object.fromEntries(files), verdict: 'passed' });
  const existing = command(binary, ['storage', 'archive', source, archive, '--json']);
  const unchanged = readFileSync(archive).equals(bytes);
  report.cases.push({ name: 'no-overwrite', status: existing.status, unchanged,
    verdict: existing.status !== successExit && unchanged && existing.stderr.includes('refusing to overwrite archive') ? 'passed' : 'failed' });
  for (const [field, value] of Object.entries({ entries: fewerNames.length,
    path_bytes: Math.min(...nameSizes), member_bytes: Math.min(...sizes), total_bytes: Math.max(...sizes) })) {
    refused(`over-${field}`, { WC_STORAGE_ARCHIVE_LIMITS: JSON.stringify({ ...limits, [field]: value }) }, `storage.archive_limits.${field}`);
  }
  refused('malformed-override', { WC_STORAGE_ARCHIVE_LIMITS: '{' }, 'WC_STORAGE_ARCHIVE_LIMITS');
  refused('null-override', { WC_STORAGE_ARCHIVE_LIMITS: 'null' }, 'storage.archive_limits');
  refused('unknown-bound', { WC_STORAGE_ARCHIVE_LIMITS: JSON.stringify({ ...limits, unused: true }) }, 'storage.archive_limits');
  refused('invalid-bound', { WC_STORAGE_ARCHIVE_LIMITS: JSON.stringify({ ...limits, entries: false }) }, 'storage.archive_limits');
  for (const field of Object.keys(limits)) {
    const incomplete = { ...limits };
    delete incomplete[field];
    refused(`missing-${field}`, { WC_STORAGE_ARCHIVE_LIMITS: JSON.stringify(incomplete) }, 'storage.archive_limits');
  }
  const link = join(source, 'link.txt');
  symlinkSync(join(source, names.values().next().value), link);
  refused('symlink-member', {}, 'unsupported symlink');
  rmSync(link);
  const linkRoot = join(output, 'linked-source');
  symlinkSync(source, linkRoot);
  refused('symlink-root', {}, 'archive source must be a real directory', linkRoot);
  const stored = readFileSync(config);
  const invalidWrite = command(binary, ['config', 'set', 'storage.archive_limits', JSON.stringify({ ...limits, entries: false })]);
  const configurationUnchanged = readFileSync(config).equals(stored);
  report.cases.push({ name: 'invalid-write-is-atomic', status: invalidWrite.status, configuration_unchanged: configurationUnchanged,
    verdict: invalidWrite.status !== successExit && configurationUnchanged && invalidWrite.stderr.includes('storage.archive_limits') ? 'passed' : 'failed' });
  assert.ok(report.cases.every(row => row.verdict === 'passed'), 'Archive qualification failed; inspect retained cases');
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  rmSync(home, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  report.scope = 'Real local CLI packing, extraction, persisted configuration and safe refusals; not a Desktop or fleet run';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') throw new Error(report.error);
