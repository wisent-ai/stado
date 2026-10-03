import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expectedRevision, snapshotSubject, verifyRevision } from '../native/subject.mjs';

// Exercise real CLI policy and a dedicated declared host. The immutable baseline
// supplies its former approvals; private account identities never live in this test.
// No baseline approval is executed, and rejected candidate calls name a fresh,
// undeclared target so a policy regression cannot start an account operation.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = join(root, '.build');
mkdirSync(build, { recursive: true });
const output = mkdtempSync(join(build, 'host-exec-privacy-'));
chmodSync(output, 0o700);
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
let environment = process.env;
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.equal(answer.status, 0, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function privatePath(path) {
  assert.ok(path, 'a dedicated fixture path is required');
  const resolved = realpathSync(path);
  const within = relative(realpathSync(build), resolved);
  assert.ok(within && !isAbsolute(within) && !within.startsWith('..'), 'fixture paths must be inside .build');
  assert.equal(statSync(resolved).mode & 0o077, 0, 'fixture paths must be owner-only');
  return resolved;
}
function snapshot(binary, name, revision) {
  assert.match(revision, /^[0-9a-f]{40}$/);
  const directory = join(output, name);
  mkdirSync(directory, { mode: 0o700 });
  const subject = snapshotSubject(binary, directory);
  report[name] = subject;
  subject.version = success(run(subject.path, ['--version']));
  subject.source_revision = verifyRevision(subject.version, revision);
  return subject;
}
function refused(answer) {
  assert.notEqual(answer.status, 0, 'a retired approval unexpectedly succeeded');
  assert.equal(answer.error, undefined, 'the native executable must actually run');
  assert.equal(answer.signal, null, 'a terminated process is not a policy refusal');
  assert.match(answer.stderr, /\[refused\]|error_code=refused/, 'require the explicit policy classification');
  assert.doesNotMatch(answer.stderr, /retryable=true/, 'an approval refusal is not retryable');
}
function catalog(binary, target) {
  const answer = run(binary, ['host', 'exec', target, '--json']);
  refused(answer);
  const line = answer.stderr.split('\n').find(value => value.includes('approved commands: '));
  assert.ok(line, 'the native refusal must carry its actual approved spellings');
  return line.split('approved commands: ')[1].split(', ');
}
function hostname(binary, fixture) {
  const receipt = JSON.parse(success(run(binary,
    ['host', 'exec', fixture.target, '--json', '--', 'hostname', '-f'])));
  assert.equal(receipt.schema, 'stado.host-exec-receipt.v1');
  assert.equal(receipt.target, fixture.target);
  assert.equal(receipt.status, 'ok');
  assert.equal(receipt.exit_code, 0);
  assert.equal(receipt.stdout.trim(), fixture.expected_hostname);
  return receipt;
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  success(run('git', ['diff', '--exit-code', 'HEAD', '--', 'stado-rs/src', 'tests/host_exec', 'tests/native']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const fixtureBytes = readFileSync(privatePath(process.env.STADO_HOST_EXEC_FIXTURE));
  report.fixture_sha256 = digest(fixtureBytes);
  const fixture = JSON.parse(fixtureBytes);
  assert.equal(fixture.dedicated, true, 'only an explicitly dedicated host fixture is accepted');
  assert.ok(fixture.target && fixture.expected_hostname && fixture.baseline_binary && fixture.binary);
  environment = { PATH: process.env.PATH, NO_COLOR: '1',
    HOME: privatePath(fixture.home), STADO_CONFIG: privatePath(fixture.config) };
  report.baseline = snapshot(fixture.baseline_binary, 'baseline', fixture.baseline_revision);
  report.candidate = snapshot(fixture.binary, 'candidate', expectedRevision(report.source_revision));
  const absentTarget = `qualification-${randomUUID()}`;
  const before = catalog(report.baseline.path, absentTarget);
  const signIns = before.filter(command => command.startsWith('start-with-skarbiec subscription sign-in '));
  assert.deepEqual(new Set(signIns.map(command => command.split(' ')[3])),
    new Set(['codex', 'claude-code', 'kimi']), 'the baseline must expose all three retired provider approvals');
  const probes = before.filter(command => command.startsWith('start-with-skarbiec test ')
    && command.split(' ').includes('--allow-provider-cost'));
  assert.equal(probes.length, 1, 'the baseline must expose its former chargeable approval');
  const retired = [...signIns, ...probes];
  const current = catalog(report.candidate.path, absentTarget);
  for (const command of retired) assert.ok(!current.includes(command), `still approved: ${command}`);
  report.before = hostname(report.candidate.path, fixture);
  report.retired_commands = retired;
  for (const command of retired) {
    refused(run(report.candidate.path,
      ['host', 'exec', absentTarget, '--json', '--', ...command.split(' ')]));
  }
  report.after = hostname(report.candidate.path, fixture);
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  report.error_code = error.code ?? null;
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Real baseline policy discovery, candidate policy refusals and dedicated host identity reads; no sign-in, model request, HTTP or graphical qualification';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
