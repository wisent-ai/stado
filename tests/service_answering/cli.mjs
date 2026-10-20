import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Qualify the installed host's real beacon through Stado's host and managed
// service reads. The chosen service must declare service.answering in the
// registry and the host must publish its answering reading. Nothing is
// created or changed on the host; a missing declaration or beacon is a
// failed prerequisite, never a fabricated successful probe.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
const binary = process.env.STADO_BIN;
const revision = process.env.STADO_TEST_SOURCE_REVISION;
const host = process.env.STADO_ANSWERING_HOST;
const name = process.env.STADO_ANSWERING_NAME;
assert.ok(binary && revision && host && name,
  'STADO_BIN, STADO_TEST_SOURCE_REVISION, STADO_ANSWERING_HOST and STADO_ANSWERING_NAME are required');
const area = mkdtempSync(join(build, 'service-answering-cli-'));
const report = { sourceRevision: revision, host, service: name, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exitStatus: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(answer.status !== null && !answer.status, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function answered(reading, origin) {
  const outcome = reading?.state === 'finished' ? reading.result
    : reading?.state === 'waiting' ? reading.last : null;
  assert.equal(outcome?.state, 'answered',
    `${origin} has no actual answered API request: ${JSON.stringify(reading)}`);
  assert.ok(Number.isInteger(outcome.status),
    `${origin} answered without an HTTP status: ${JSON.stringify(reading)}`);
  assert.equal(outcome.successful, true,
    `${origin} answered HTTP ${outcome.status} without a successful readiness verdict: ${outcome.detail ?? JSON.stringify(reading)}`);
}
let failure;
try {
  assert.equal(success(run('git', ['rev-parse', 'HEAD'])), revision);
  assert.equal(success(run('git', ['status', '--porcelain'])), '', 'qualify a clean committed revision');
  const executable = binary.includes('/') ? resolve(root, binary) : success(run('which', [binary]));
  report.binary = { path: executable, sha256: digest(readFileSync(executable)),
    version: success(run(executable, ['--version'])) };
  report.testSha256 = digest(readFileSync(fileURLToPath(import.meta.url)));

  const health = JSON.parse(success(run(executable, ['host', 'health', host, '--json'])));
  const ping = JSON.parse(success(run(executable, ['host', 'ping', host, '--json'])));
  assert.equal(ping.status, 'ok', `host ping did not establish a fresh beacon: ${JSON.stringify(ping)}`);
  const status = JSON.parse(success(run(executable, ['service', 'status', name, '--json'])));
  const row = status.find(entry => entry.host === host && entry.name === name);
  assert.ok(row, `the registry reports no ${name} on ${host}`);
  const observed = health.beacon?.units?.[row.unit_id]?.answering;
  answered(observed, `the real host beacon for ${row.unit_id}`);
  answered(row.answering, `service status for ${row.unit_id}`);
  assert.ok(row.reported_at, 'service status omitted the beacon observation time');

  const listed = JSON.parse(success(run(executable, ['service', 'list', '--json'])));
  const listedRow = listed.find(entry => entry.host === host && entry.unit_id === row.unit_id);
  answered(listedRow?.answering, `service list for ${row.unit_id}`);
  const plain = success(run(executable, ['service', 'status', name]));
  const prefix = `answering: ${host} ${row.unit_id}: `;
  assert.ok(plain.split('\n').some(line => line.startsWith(prefix) && line.includes('"state"')),
    'the plain status table omitted the API-question state');

  const missing = `unmanaged-${randomUUID()}`;
  const refused = run(executable, ['service', 'status', missing, '--json']);
  assert.ok(refused.status !== null && refused.status, 'an undeclared unit was accepted');
  assert.ok(refused.stderr.includes(`no registry-managed service named ${missing}`), refused.stderr);
  report.observed = { ping, beacon: health.beacon?.reported_at, hostQuestion: observed,
    statusQuestion: row.answering, listQuestion: listedRow.answering, refusal: refused.stderr };
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finishedAt = new Date().toISOString();
  writeFileSync(join(area, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(area, 'report.json')}`);
}
if (failure) throw failure;
