// Run on an isolated release host with a real failed predecessor and a different
// desired release. The fixture is retained from the product's release flow;
// this journey never manufactures quarantine evidence or a provider response.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../../native/process.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const build = realpathSync(join(root, '.build'));
function inside(parent, path) {
  const child = realpathSync(path);
  const suffix = relative(parent, child);
  assert.ok(suffix && !isAbsolute(suffix) && suffix !== '..' && !suffix.startsWith(`..${sep}`),
    `${child} must be inside ${parent}`);
  return child;
}
const home = inside(build, required('STADO_TEST_QUARANTINE_HOME'));
const config = inside(home, required('STADO_TEST_QUARANTINE_CONFIG'));
const statePath = inside(home, required('STADO_TEST_QUARANTINE_STATE'));
const binary = realpathSync(required('STADO_BIN'));
const store = inside(home, required('STADO_TEST_QUARANTINE_STORE'));
const temporary = inside(home, required('STADO_TEST_QUARANTINE_TMP'));
const environment = {
  PATH: required('PATH'), HOME: home, STADO_CONFIG: config, TMPDIR: temporary,
  WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: store,
};
const target = required('STADO_TEST_QUARANTINE_TARGET');
const product = required('STADO_TEST_QUARANTINE_PRODUCT');
const expectedCause = required('STADO_TEST_QUARANTINE_CAUSE');
const output = join(build, 'release-quarantine', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = {
  source_revision: required('STADO_TEST_SOURCE_REVISION'),
  binary, binary_sha256: digest(readFileSync(binary)),
  journey_sha256: digest(readFileSync(fileURLToPath(import.meta.url))),
  started_at: new Date().toISOString(), commands: [], verdict: 'failed',
};
const readState = () => JSON.parse(readFileSync(statePath, 'utf8'));
function stado(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8', env: environment });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr,
    error: result.error?.message });
  assert.ifError(result.error);
  assert.ok(exitedCleanly(result), result.stderr);
  return JSON.parse(result.stdout);
}
try {
  const { policy } = stado(['release', 'policy', 'show', product, '--json']);
  const declared = policy.targets[target];
  assert.ok(declared, 'the isolated registry must declare the tested target');
  assert.equal(realpathSync(declared.home), home);
  for (const path of [declared.state_dir, declared.runtime_root, declared.logs_root, policy.install_root]) {
    inside(home, path);
  }
  assert.equal(realpathSync(join(declared.state_dir, `${product}.json`)), statePath);
  const desiredDigest = policy.desired.artifacts[declared.platform].artifact_sha256;
  assert.ok(desiredDigest, 'the real registry must declare the next candidate');
  report.policy = policy;
  const before = readState();
  report.before = before;
  assert.equal(before.product, product);
  assert.equal(before.target, target);
  assert.equal(before.phase, 'quarantined', 'the real predecessor must already be quarantined');
  const entries = Object.entries(before.quarantined);
  const [entry, ...others] = entries;
  assert.ok(entry, 'the fixture has no real quarantine');
  assert.deepEqual(others, [], 'this boundary requires a single failed predecessor');
  const [predecessorDigest, predecessor] = entry;
  assert.equal(predecessor.cause, expectedCause);
  assert.notEqual(predecessorDigest, desiredDigest, 'the next candidate must have different bytes');
  assert.equal(before.active, null);
  assert.equal(before.previous, null);
  assert.equal(before.candidate, null);
  const states = stado(['release', 'agent', '--target', target, '--product', product, '--json']);
  const after = readState();
  report.after = after;
  assert.equal(after.phase, 'quarantined', 'a retained candidate cause must hold immediately');
  assert.deepEqual(after.quarantined, before.quarantined, 'admission must preserve the failure evidence');
  assert.equal(after.candidate, null, 'no candidate may start while recovery is unproved');
  assert.equal(after.active, null);
  assert.equal(after.previous, null);
  assert.ok(after.detail.includes(expectedCause), after.detail);
  assert.ok(after.detail.includes('recovery has not been established'), after.detail);
  const state = states.find(value => value.product === product && value.target === target);
  assert.deepEqual(state, after, 'CLI and persisted release state must agree');
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack;
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
