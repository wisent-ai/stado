import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado credentials get <role>` refusing a role no
// visible item carries: not found, the consumer and the tag stado:role:<role>
// named, the retag command offered, no item name made of a sentence and no
// --field hint. Read-only: the role is a fresh random name no item can carry.
// Required: STADO_BIN, the Stado under test (a path or a program on PATH),
// and the machine's own configured vault (the one `stado credentials vault show`
// names).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN must name the Stado under test, a path or a program on PATH');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-role-holder-'));
const role = `qualification-unheld-${randomUUID()}`;
const tag = `stado:role:${role}`;
const report = { started_at: new Date().toISOString(), role, commands: [], result: 'failed' };
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
  report.vault = success(run(binary, [...'credentials vault'.split(' '), 'show']));

  const refused = run(binary, ['credentials', 'get', role]);
  assert.ok(!succeeded(refused), `a role no item carries was answered: ${refused.stdout}`);
  assert.equal(refused.stdout, '', 'a refusal wrote to standard output');
  const said = refused.stderr;
  assert.ok(said.includes(`carries ${tag}`), `the refusal does not name ${tag}: ${said}`);
  assert.ok(said.includes('stado credentials item retag'), `the refusal offers no retag: ${said}`);
  assert.ok(said.includes('not_found'), `the refusal is not classified not found: ${said}`);
  assert.ok(!said.includes('has no value'), `the refusal still reads as an item without a value: ${said}`);
  assert.ok(!said.includes('--field'), `the refusal still sends the reader to --field: ${said}`);
  report.refusal = said;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'stado credentials get on a role no visible item carries, against the machine\'s configured vault, read-only';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (report.error) {
  throw new Error(`role-holder qualification failed; report in ${output}: ${report.error}`);
}
