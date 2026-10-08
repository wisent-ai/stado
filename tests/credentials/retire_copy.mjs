import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado credentials vault retire` on a machine
// that reads the vault owner and holds no vault of its own. Read-only: no
// run here passes --apply, so nothing is moved and nothing is removed.
//
// - The report on a real vault copy compares it with the owner item by item,
//   exits successfully, says nothing was removed, and leaves the file
//   byte-for-byte as it was.
// - A vault of another owner is refused as a different vault, not a copy.
// - A path that holds no file is refused as not found.
//
// Required: STADO_BIN (the Stado under test, a path or a program on PATH),
// STADO_VAULT_OWNER (the registry host holding the fleet vault),
// RETIRE_COPY_PATH (a fleet vault copy on this machine) and
// RETIRE_FOREIGN_PATH (a vault file on this machine with another owner).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value, `${name} must be set; the header of ${fileURLToPath(import.meta.url)} says what it names`);
  return value;
};
const stado = required('STADO_BIN');
const owner = required('STADO_VAULT_OWNER');
const copy = required('RETIRE_COPY_PATH');
const foreign = required('RETIRE_FOREIGN_PATH');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-retire-copy-'));
const report = { started_at: new Date().toISOString(), owner, copy, foreign, commands: [], result: 'failed' };
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
function refusal(answer, words, label) {
  assert.ok(!succeeded(answer), `${label} was not refused: ${answer.stdout}`);
  assert.ok(answer.stderr.includes(words), `${label} was refused without naming "${words}": ${answer.stderr}`);
  return answer.stderr;
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const vault = 'credentials vault'.split(' ');
  report.vault = success(run(binary, [...vault, 'show']));

  const before = digest(readFileSync(copy));
  const compared = JSON.parse(success(run(binary,
    [...vault, 'retire', copy, '--owner', owner, '--json'])));
  assert.equal(compared.copy, copy, 'the report does not name the copy it read');
  assert.equal(typeof compared.held_by_owner, 'number', 'the report carries no held count');
  assert.ok(Array.isArray(compared.missing_on_owner), 'the report carries no missing list');
  for (const item of compared.missing_on_owner) {
    assert.ok(['only_in_copy', 'newer_in_copy'].includes(item.reason), `unexplained reason for ${item.id}: ${item.reason}`);
  }
  assert.deepEqual(compared.moved, [], 'a report-only run moved items');
  assert.equal(compared.removed, false, 'a report-only run removed the copy');
  assert.equal(digest(readFileSync(copy)), before, 'a report-only run changed the copy');
  report.compared = { held_by_owner: compared.held_by_owner, missing_on_owner: compared.missing_on_owner.length };

  report.foreign_refusal = refusal(run(binary,
    [...vault, 'retire', foreign, '--owner', owner]),
    'it is a different vault, not a copy of the fleet\'s', 'a vault of another owner');

  const absent = join(output, `no-vault-${randomUUID()}.json`);
  report.absent_refusal = refusal(run(binary,
    [...vault, 'retire', absent, '--owner', owner]),
    `no vault file at ${absent}`, 'a path with no file');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'stado credentials vault retire without --apply: a real copy compared with the real owner, and two refusals';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (report.error) {
  throw new Error(`retire-copy qualification failed; report in ${output}: ${report.error}`);
}
