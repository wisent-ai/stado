// Real `release delivery worker` CLI refusals before release-store access or delivery execution.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'delivery-job-identity', randomUUID());
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = { result: 'failed', commands: [], cases: [], journey_sha256: digest(readFileSync(fileURLToPath(import.meta.url))) };
function save() { writeFileSync(join(output, 'report.json'), JSON.stringify(report)); }
function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must be explicitly declared`);
  return value;
}
function command(program, args, env, cwd) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal, start_error: result.error?.message, stdout: result.stdout, stderr: result.stderr });
  save();
  if (result.error) throw result.error;
  assert.equal(result.signal, null, `${program} terminated by ${result.signal}`);
  return result;
}
save();
try {
  const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
  assert.ok(Number.isInteger(successExit), 'STADO_TEST_SUCCESS_EXIT must be an integer');
  report.success_exit = successExit;
  const binary = realpathSync(required('STADO_BIN'));
  const revision = required('STADO_TEST_SOURCE_REVISION');
  const requestPath = realpathSync(required('STADO_TEST_DELIVERY_REQUEST'));
  const requestBytes = readFileSync(requestPath);
  const request = JSON.parse(requestBytes);
  assert.equal(typeof request.run_id, 'string', 'use an actual retained delivery request');
  assert.equal(typeof request.source_sha256, 'string', 'the retained request must bind its real source');
  report.binary = binary;
  report.binary_sha256 = digest(readFileSync(binary));
  report.source_revision = revision;
  report.request = { original: requestPath, sha256: digest(requestBytes), run_id: request.run_id, source_sha256: request.source_sha256 };
  const source = command('git', ['rev-parse', 'HEAD'], { PATH: required('PATH') }, root);
  assert.equal(source.status, successExit, source.stderr);
  assert.equal(source.stdout.trim(), revision, 'the journey checkout must match the candidate revision');
  const patch = command('git', ['diff', '--binary', 'HEAD'], { PATH: required('PATH') }, root);
  assert.equal(patch.status, successExit, patch.stderr);
  writeFileSync(join(output, 'source.patch'), patch.stdout);
  assert.equal(patch.stdout, '', 'tracked source must be committed before qualification');
  const version = command(binary, ['--version'], { PATH: required('PATH'), HOME: output }, output);
  assert.equal(version.status, successExit, version.stderr);
  assert.ok(version.stdout.trim().endsWith(`(rev ${revision})`), `candidate embedded revision differs: ${version.stdout}`);
  report.binary_version = version.stdout.trim();
  const requestFailures = join(output, 'request-failures');
  const requestHome = join(requestFailures, 'home');
  mkdirSync(requestHome, { recursive: true });
  const requestEnvironment = { PATH: required('PATH'), HOME: requestHome, STADO_CONFIG: join(requestHome, 'config.json') };
  const absentRequest = join(requestFailures, 'absent-request.json');
  let missingErrno;
  try {
    readFileSync(absentRequest);
  } catch (error) {
    missingErrno = Math.abs(error.errno);
    report.missing_request_cause = { code: error.code, errno: error.errno, message: error.message };
  }
  assert.ok(Number.isInteger(missingErrno), 'the isolated request must produce a real filesystem refusal');
  const missing = command(binary, ['release', 'delivery', 'worker', '--request', absentRequest], requestEnvironment, requestFailures);
  assert.notEqual(missing.status, null, 'unreadable request must produce an actual refusal');
  assert.notEqual(missing.status, successExit, 'an absent request cannot admit delivery');
  assert.ok(missing.stderr.includes(`cannot read delivery request ${absentRequest}:`), missing.stderr);
  assert.ok(missing.stderr.includes(`(os error ${missingErrno})`), `filesystem cause was discarded: ${missing.stderr}`);
  const malformedRequest = join(requestFailures, 'malformed-request.json');
  const malformedBytes = Buffer.from('{');
  writeFileSync(malformedRequest, malformedBytes);
  const malformed = command(binary, ['release', 'delivery', 'worker', '--request', malformedRequest], requestEnvironment, requestFailures);
  assert.notEqual(malformed.status, null, 'malformed request must produce an actual refusal');
  assert.notEqual(malformed.status, successExit, 'malformed JSON cannot admit delivery');
  assert.ok(malformed.stderr.includes(`cannot decode delivery request ${malformedRequest}:`), malformed.stderr);
  assert.ok(malformed.stderr.includes('EOF'), `decoder cause was discarded: ${malformed.stderr}`);
  assert.deepEqual(readFileSync(malformedRequest), malformedBytes, 'refusal must preserve invalid input');
  assert.equal(existsSync(absentRequest), false, 'refusal cannot create the missing request');
  assert.equal(existsSync(join(requestFailures, 'output')), false, 'request refusals cannot create delivery output');
  assert.equal(existsSync(join(requestHome, '.stado')), false, 'request refusals must precede queue-store access');
  report.request_refusals = { missing: 'passed', malformed: 'passed', inputs_unchanged: true, output_absent: true, queue_store_absent: true };
  save();
  const cases = [
    { name: 'unset', environment: {}, cause: 'cannot read WC_JOB_ID' },
    { name: 'empty', environment: { WC_JOB_ID: '' }, cause: 'empty WC_JOB_ID' },
    { name: 'whitespace', environment: { WC_JOB_ID: ' \t\n' }, cause: 'empty WC_JOB_ID' },
  ];
  for (const item of cases) {
    const fixture = join(output, item.name);
    const home = join(fixture, 'home');
    mkdirSync(home, { recursive: true });
    const retained = join(fixture, 'delivery-request.json');
    writeFileSync(retained, requestBytes);
    const env = { PATH: required('PATH'), HOME: home, STADO_CONFIG: join(home, 'config.json'), ...item.environment };
    const result = command(binary, ['release', 'delivery', 'worker', '--request', retained], env, fixture);
    assert.notEqual(result.status, null, 'worker must exit with its actual refusal');
    assert.notEqual(result.status, successExit, 'missing queue identity cannot admit a delivery');
    assert.ok(result.stderr.includes(item.cause), `wrong operation/cause: ${result.stderr}`);
    assert.ok(result.stderr.includes(retained), `refusal must identify the actual request: ${result.stderr}`);
    assert.deepEqual(readFileSync(retained), requestBytes, 'refusal must preserve the request');
    assert.equal(existsSync(join(fixture, 'output')), false, 'refusal cannot create delivery output or a receipt');
    assert.equal(existsSync(join(home, '.stado')), false, 'refusal must precede local queue-store initialization');
    report.cases.push({ name: item.name, result: 'passed', request_unchanged: true, output_absent: true, queue_store_absent: true });
    save();
  }
  assert.deepEqual(readFileSync(requestPath), requestBytes, 'the original retained request must remain untouched');
  report.result = 'passed';
} catch (error) {
  report.failure = error.stack;
  throw error;
} finally {
  save();
}
