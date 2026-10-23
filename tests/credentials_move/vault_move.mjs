import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado credentials vault move` against a real fleet,
// without moving anything:
//
// - Without --apply, the report reads the placement profile that carries
//   Skarbiec, the owner host, the destination, the vault, the current and
//   the new owner identity, whether the new owner is registered, whether the
//   destination's keyring holds its secret half, and the live item count; it
//   exits successfully, says it applied nothing, and a second reading finds
//   the same owner host, owner identity and live items.
// - With --apply and an owner identity the destination's keyring does not
//   hold, the move is refused before any change, naming the missing secret
//   half, and a reading afterwards finds the vault where it was.
//
// The --apply path that rotates and moves the vault needs a fleet of its own
// whose vault may be moved; it is not run here, and the report says so.
//
// Required: STADO_BIN (the Stado under test), VAULT_MOVE_TO (the registered
// host that would become the owner) and VAULT_MOVE_OWNER_UID (an identity
// whose secret half that host's keyring holds).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value, `${name} must be set; the header of ${fileURLToPath(import.meta.url)} says what it names`);
  return value;
};
const stado = required('STADO_BIN');
const to = required('VAULT_MOVE_TO');
const owner = required('VAULT_MOVE_OWNER_UID');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-vault-move-'));
const report = { started_at: new Date().toISOString(), to, owner, commands: [], result: 'failed' };
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
// The verb under test, as its command line spells it.
const move = 'credentials vault move'.split(' ');
function read() {
  return JSON.parse(success(run(binary(), [...move, '--to', to, '--owner-uid', owner, '--json'])));
}
let resolved;
function binary() {
  resolved ??= stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  return resolved;
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: binary(), sha256: digest(readFileSync(binary())), version: success(run(binary(), ['--version'])) };

  const planned = read();
  assert.equal(planned.applied, false, 'a run without --apply says it applied something');
  assert.equal(planned.to_host, to, 'the report names another destination');
  assert.equal(planned.owner_uid, owner, 'the report names another owner identity');
  assert.ok(planned.services.includes('skarbiec'), `the profile ${planned.profile} carries no skarbiec`);
  assert.equal(planned.destination_secret_key, 'present', `the destination's keyring lacks ${owner}`);
  assert.ok(Number.isInteger(planned.live_items) && planned.live_items > 0, 'the report counts no live items');
  assert.ok(!('moved' in planned), 'a run without --apply carries a move result');
  report.planned = planned;

  const again = read();
  assert.equal(again.from_host, planned.from_host, 'the owner host changed under a run without --apply');
  assert.equal(again.previous_owner, planned.previous_owner, 'the owner identity changed under a run without --apply');
  assert.equal(again.live_items, planned.live_items, 'the live items changed under a run without --apply');

  const absent = `vault-move-absent-${randomUUID()}`;
  const refused = run(binary(), [...move, '--to', to, '--owner-uid', absent, '--apply']);
  assert.ok(!succeeded(refused), `--apply with an identity the destination lacks was not refused: ${refused.stdout}`);
  assert.ok(refused.stderr.includes(`holds no secret half for ${absent}`),
    `the refusal does not name the missing secret half: ${refused.stderr}`);
  const after = read();
  assert.equal(after.from_host, planned.from_host, 'the refused --apply moved the vault');
  assert.equal(after.previous_owner, planned.previous_owner, 'the refused --apply rotated the vault');
  report.refusal = refused.stderr;
  report.not_run = '--apply on a vault that may be moved: it rotates and moves the fleet vault, so it runs only on a fleet of its own';
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'stado credentials vault move without --apply on a real fleet, and the missing-secret refusal of --apply';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (report.error) {
  throw new Error(`vault-move qualification failed; report in ${output}: ${report.error}`);
}
