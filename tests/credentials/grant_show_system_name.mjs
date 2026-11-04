import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of a host named by this machine's own system name:
// `stado credentials grant show --host "$(hostname -s)" <consumer>` reaches
// the registry entry `stado registry self` names for this machine, so its
// answer is the same as with --host <that entry>, and a name no target is
// called or declares is still refused as not found, naming it. Read-only:
// grant show reads the vault's grant list and writes nothing.
// Required: STADO_BIN, the Stado under test (a path or a program on PATH),
// on a machine whose registry entry `stado registry self` names, and
// STADO_TEST_GRANT_CONSUMER, a consumer whose grant that entry's vault holds.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN must name the Stado under test, a path or a program on PATH');
const consumer = process.env.STADO_TEST_GRANT_CONSUMER;
assert.ok(consumer, 'STADO_TEST_GRANT_CONSUMER must name a consumer whose grant this machine\'s vault holds');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-grant-show-system-name-'));
const report = { started_at: new Date().toISOString(), consumer, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const succeeded = answer => answer.status !== null && !answer.status;
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: process.env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const system = success(run('hostname', ['-s']));
  const entry = success(run(binary, ['registry', 'self', '--name-only']));
  report.system_name = system;
  report.registry_entry = entry;
  assert.notEqual(system, entry, 'this machine\'s system name is its registry name, so the case proves nothing');

  const byEntry = JSON.parse(success(run(binary, ['credentials', 'grant', 'show', '--host', entry, consumer, '--json'])));
  const bySystem = JSON.parse(success(run(binary, ['credentials', 'grant', 'show', '--host', system, consumer, '--json'])));
  assert.deepEqual(bySystem, byEntry, 'the system name and the registry entry answered different grants');

  const unknown = `qualification-unregistered-${randomUUID()}`;
  const refused = run(binary, ['credentials', 'grant', 'show', '--host', unknown, consumer, '--json']);
  assert.ok(!succeeded(refused), `a host nobody declares was answered: ${refused.stdout}`);
  assert.ok(refused.stderr.includes(unknown), `the refusal does not name the host: ${refused.stderr}`);
  assert.ok(refused.stderr.includes('not_found'), `the refusal is not classified not found: ${refused.stderr}`);
  report.refusal = refused.stderr;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'stado credentials grant show by this machine\'s system name and by an undeclared name, read-only';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
  process.exitCode = report.result === 'passed' ? 0 : 1;
}
