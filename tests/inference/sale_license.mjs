import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// A deployment sold outside the fleet is not applied while its model declares
// no license: the real `stado inference apply` of a saved plan whose model
// chain declares none (or whose license was not read) is refused once a
// priced caller of the deployment exists, the refusal naming the caller and
// what the license lacks, and the host is not touched; the caller is revoked
// again. STADO_BIN selects the Stado under test; STADO_TEST_UNLICENSED_PLAN
// a plan id `stado inference plan` saved for such a model, and
// STADO_TEST_UNLICENSED_DEPLOYMENT its deployment name (the operator's,
// nothing assumed). Without them the test is reported blocked.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'inference-sale-license-'));
const stado = process.env.STADO_BIN;
const planId = process.env.STADO_TEST_UNLICENSED_PLAN;
const deployment = process.env.STADO_TEST_UNLICENSED_DEPLOYMENT;
const caller = `sale-license-test-${process.pid}`;
const report = { started_at: new Date().toISOString(), plan_id: planId, deployment, caller, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(exitedCleanly(answer), JSON.stringify({ stderr: answer.stderr, error: answer.error?.message,
    signal: answer.signal, status: answer.status }));
  return answer.stdout.trim();
}
let added = false;
let binary;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado || !planId || !deployment) {
    report.result = 'blocked';
    throw new Error('requires STADO_BIN, STADO_TEST_UNLICENSED_PLAN and STADO_TEST_UNLICENSED_DEPLOYMENT');
  }
  report.stado = snapshotSubject(stado, output);
  binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);

  success(run(binary, ['inference', 'callers', 'add', deployment, '--caller', caller,
    '--requests-per-minute', '1', '--tokens-per-day', '1',
    '--usd-per-million-prompt', '1', '--usd-per-million-generated', '1', '--json']));
  added = true;
  const applied = run(binary, ['inference', 'apply', planId, '--json']);
  assert.ok(!exitedCleanly(applied), `applied: ${applied.stdout}`);
  const said = `${applied.stderr}${applied.stdout}`;
  assert.ok(said.includes('sold outside the fleet'), `the refusal is not the license gate: ${said}`);
  assert.ok(said.includes(`caller ${caller}`), `the refusal does not name the caller: ${said}`);
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  if (added) run(binary, ['inference', 'callers', 'revoke', deployment, '--caller', caller, '--json']);
  report.finished_at = new Date().toISOString();
  report.scope = 'inference apply refusing a sold deployment whose model declares no license';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
