// Inspect real isolated fleet handoffs; never manufacture receipts or queue work.
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
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = realpathSync(required('STADO_BIN'));
const revision = required('STADO_TEST_SOURCE_REVISION');
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const buildOnly = required('STADO_TEST_BUILD_ONLY_CHANGE');
const behavioral = required('STADO_TEST_BEHAVIORAL_CHANGE');
assert.notEqual(buildOnly, behavioral, 'fixtures must be distinct real handoffs');
const output = join(root, '.build', 'behavioral-qualification', randomUUID());
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = {
  started_at: new Date().toISOString(), revision, binary,
  binary_sha256: digest(readFileSync(binary)),
  test_sha256: digest(readFileSync(fileURLToPath(import.meta.url))),
  commands: [], verdict: 'failed',
};
function command(program, args) {
  const result = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal,
    error: result.error?.message, stdout: result.stdout, stderr: result.stderr });
  assert.equal(result.error, undefined, `cannot execute ${program}: ${result.error?.message}`);
  assert.equal(result.status, successExit, `${program} ${args.join(' ')} failed: ${result.stderr}`);
  return result.stdout;
}
function row(id) {
  const rows = JSON.parse(command(binary, ['release', 'changes', 'list', '--id', id, '--json']));
  assert.ok(Array.isArray(rows), 'change listing must be an array');
  const [value, ...duplicates] = rows.filter(candidate => candidate.id === id);
  assert.ok(value, `real handoff ${id} is absent`);
  assert.deepEqual(duplicates, [], `handoff ${id} is duplicated`);
  assert.equal(value.qualification_policy, 'required-behavioral-tests-v1');
  assert.ok(value.run_id, `handoff ${id} has no actual build`);
  assert.ok(Array.isArray(value.evidence) && value.evidence.some(receipt => receipt.build.status === 'passed'),
    `handoff ${id} has no retained passing build receipt`);
  for (const receipt of value.evidence) {
    assert.equal(receipt.run_id, value.run_id);
    assert.equal(receipt.product, value.product);
    assert.equal(receipt.build.status, 'passed');
    assert.equal(receipt.build.exit_code, successExit);
    assert.ok(receipt.source_commit && receipt.source_sha256 && receipt.manifest_sha256,
      'build evidence must retain source and manifest identities');
  }
  return value;
}
try {
  assert.equal(command('git', ['rev-parse', 'HEAD']).trim(), revision);
  command('git', ['diff', '--exit-code', 'HEAD']);
  const missing = row(buildOnly);
  assert.equal(missing.state, 'awaiting_tests', 'a passing build without behavioral tests must not qualify work');
  assert.ok(missing.failure?.includes('no complete passing post-build test evidence'),
    'the refusal must explain missing behavioral test evidence');
  assert.ok(missing.evidence.every(receipt => receipt.quality.every(step => !step.name.startsWith('test:'))),
    'build-only fixture must actually contain no recorded behavioral tests');
  const qualified = row(behavioral);
  assert.equal(qualified.state, 'passed');
  const tests = qualified.evidence.flatMap(receipt => receipt.quality.filter(step => step.name.startsWith('test:')));
  assert.ok(tests.some(test => test.status === 'passed'), 'passing fixture must contain actual post-build tests');
  for (const test of tests) {
    assert.equal(test.status, 'passed');
    assert.equal(test.exit_code, successExit);
    assert.ok(Array.isArray(test.argv) && test.argv.some(argument => typeof argument === 'string'),
      'retain the actual test command');
  }
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
