// Real configuration writes and refusal atomicity; this does not qualify certificate issuance.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = join(root, 'build', 'database-tls-policy');
mkdirSync(build, { recursive: true });
const output = mkdtempSync(join(build, 'run-'));
const home = join(output, 'home');
mkdirSync(home);
const config = join(home, 'config.json');
const env = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { started_at: new Date().toISOString(), commands: [], cases: [], verdict: 'failed' };
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
function required(name) {
  assert.ok(process.env[name], `${name} must declare the qualification input`);
  return process.env[name];
}
function run(program, args) {
  const result = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  return result;
}
const persisted = () => JSON.parse(readFileSync(config, 'utf8'));
try {
  const binary = required('STADO_BIN');
  const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
  assert.ok(Number.isInteger(successExit), 'STADO_TEST_SUCCESS_EXIT must be an integer exit status');
  function success(result) {
    assert.equal(result.status, successExit, result.stderr || result.error?.message);
    return result.stdout.trim();
  }
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/config',
    'stado-rs/src/config_file', 'stado-rs/src/capabilities', 'tests/database']));
  report.test_sha256 = hash(readFileSync(fileURLToPath(import.meta.url)));
  const path = realpathSync(binary.includes('/') ? binary : success(run('which', [binary])));
  report.binary = { path, sha256: hash(readFileSync(path)), version: success(run(binary, ['--version'])) };
  const policy = JSON.parse(required('STADO_TEST_DATABASE_TLS_POLICY'));
  const refusals = JSON.parse(required('STADO_TEST_DATABASE_TLS_REFUSALS'));
  assert.ok(Array.isArray(refusals), 'STADO_TEST_DATABASE_TLS_REFUSALS must be an array of named invalid policy objects');
  assert.deepEqual(new Set(refusals.map(row => row.name)), new Set([
    'missing-days', 'missing-bits', 'unknown-member', 'zero-days', 'zero-bits',
    'negative-days', 'fractional-days', 'text-bits', 'null-policy',
  ]), 'The declared qualification cases must cover every policy refusal class');
  report.inputs = { successExit, policy, refusals };
  success(run(binary, ['config', 'init']));
  success(run(binary, ['config', 'set', 'database.postgres_tls', JSON.stringify(policy)]));
  assert.deepEqual(persisted().database.postgres_tls, policy);
  success(run(binary, ['config', 'validate']));
  report.cases.push({ name: 'persisted-policy', verdict: 'passed', policy });
  for (const { name, value } of refusals) {
    assert.notEqual(value, undefined, `${name} must declare its invalid policy`);
    const before = persisted();
    const result = run(binary, ['config', 'set', 'database.postgres_tls', JSON.stringify(value)]);
    const after = persisted();
    let error;
    try {
      assert.notEqual(result.status, successExit, 'Invalid policy was accepted');
      assert.match(`${result.stdout}\n${result.stderr}`, /database\.postgres_tls/);
      assert.deepEqual(after, before, 'A refused policy changed persisted configuration');
    } catch (failure) { error = String(failure); }
    report.cases.push({ name, before, after, status: result.status,
      verdict: error ? 'failed' : 'passed', error });
    // Restore through the writer so an old-binary failure cannot hide another case.
    success(run(binary, ['config', 'set', 'database.postgres_tls', JSON.stringify(policy)]));
  }
  assert.ok(report.cases.every(row => row.verdict === 'passed'), 'TLS policy cases failed; see retained report');
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  rmSync(home, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  report.scope = 'Persisted CLI configuration and refusal atomicity, not certificate issuance or GUI qualification';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') throw new Error(report.error);
