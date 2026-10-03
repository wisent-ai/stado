import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real installed Stado and Skarbiec; no service, operator vault or cloud account.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = join(root, '.build');
mkdirSync(build, { recursive: true });
// GPG's socket path must fit the host's Unix-domain socket address.
const output = mkdtempSync(join(build, 'dg-'));
const home = join(output, 'home');
const keyring = join(output, 'g');
const temporary = join(output, 'temporary');
for (const directory of [home, keyring, temporary, join(home, '.stado')]) {
  mkdirSync(directory, { recursive: true, mode: 0o700 });
}
const config = join(home, 'config.json');
const vault = join(home, 'vault.json');
const environment = {
  PATH: process.env.PATH, HOME: home, STADO_CONFIG: config, GNUPGHOME: keyring,
  TMPDIR: temporary, SKARBIEC_VAULT_FILE: vault,
  SKARBIEC_AUDIT_FILE: join(home, 'audit.jsonl'),
};
const stado = process.env.STADO_BIN || 'stado';
const skarbiec = process.env.SKARBIEC_BIN || 'skarbiec';
const report = { started_at: new Date().toISOString(), commands: [], transitions: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function run(program, args, input, retainOutput = true) {
  const result = spawnSync(program, args, {
    cwd: root, env: environment, encoding: 'utf8', input, maxBuffer: 8 * 1024 * 1024,
  });
  report.commands.push({ program, arguments: args, exit_status: result.status,
    signal: result.signal, error: result.error?.message,
    stdout: retainOutput ? result.stdout : undefined,
    stdout_sha256: result.stdout === null ? null : digest(result.stdout), stderr: result.stderr });
  return result;
}
function success(result) {
  assert.equal(result.status, 0, result.stderr || result.error?.message || result.signal);
  return result.stdout.trim();
}
function persisted() {
  return JSON.parse(readFileSync(config, 'utf8')).database_api.databases['grant-journey'];
}
function change(verb, consumer, expectedExit) {
  const before = persisted();
  const result = run(stado, ['database', verb, 'grant-journey', '--consumer', consumer, '--json']);
  const after = persisted();
  const transition = { verb, consumer, before, after, exit_status: result.status };
  report.transitions.push(transition);
  assert.equal(result.status, expectedExit, result.stderr);
  transition.result = result.stdout.trim() ? JSON.parse(result.stdout) : null;
  return transition;
}
function seedGrant(consumer) {
  const token = join(home, '.stado', `${consumer}-skarbiec-token`);
  writeFileSync(token, randomBytes(32).toString('hex'), { mode: 0o600 });
  const before = digest(readFileSync(token));
  success(run(skarbiec, ['grant', 'issue', consumer, '--capabilities', 'read:grant-seed', '--token-file', token], undefined, false));
  return { token, before };
}

try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/database', 'tests/database']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  for (const [name, binary, versionArguments] of [
    ['stado', stado, ['--version']], ['skarbiec', skarbiec, ['version']],
  ]) {
    const path = binary.includes('/') ? resolve(binary) : success(run('which', [binary]));
    report[name] = { path, sha256: digest(readFileSync(path)), version: success(run(binary, versionArguments)) };
  }
  environment.SKARBIEC_BIN = report.skarbiec.path;
  success(run(stado, ['config', 'init']));
  success(run(skarbiec, ['init', 'database-grant-journey']));
  success(run(stado, ['config', 'set', 'secrets.skarbiec.vault_file', vault]));
  for (const item of ['grant-seed', 'grant-journey-database']) {
    success(run(skarbiec, ['set-json', item], JSON.stringify({
      schema: 'skarbiec.item.v2', kind: 'bundle', context: {},
      fields: { value: randomBytes(32).toString('hex') },
    }), false));
  }
  success(run(stado, ['database', 'declare', 'grant-journey', '--engine', 'sqlite', '--consumer', 'base', '--json']));

  // A missing grant fails after a newly added declaration; a repeated attempt
  // must distinguish the unchanged declaration from the same dependency error.
  const addedButRefused = change('grant', 'reader', 1);
  const repeatedRefusal = change('grant', 'reader', 1);
  assert.deepEqual(addedButRefused.after.consumers, ['base', 'reader']);
  assert.deepEqual(repeatedRefusal.after, repeatedRefusal.before);

  const reader = seedGrant('reader');
  const settledExisting = change('grant', 'reader', 0);
  const repeatedSuccess = change('grant', 'reader', 0);
  const other = seedGrant('other');
  const addedAndSettled = change('grant', 'other', 0);
  assert.equal(digest(readFileSync(reader.token)), reader.before);
  assert.equal(digest(readFileSync(other.token)), other.before);
  assert.deepEqual(settledExisting.result.skarbiec[0].added, ['read:grant-journey-database']);
  assert.deepEqual(repeatedSuccess.result.skarbiec[0].added, []);
  assert.deepEqual(addedAndSettled.after.consumers, ['base', 'reader', 'other']);
  report.persisted_grants = JSON.parse(success(run(skarbiec, ['grant', 'list'])));
  for (const consumer of ['reader', 'other']) {
    const stored = report.persisted_grants.find(grant => grant.consumer === consumer);
    assert.ok(stored, `${consumer}: successful grant is absent from the real vault`);
    assert.deepEqual(
      new Set(stored.capabilities.map(({ action, item, field }) => [action, item, field])),
      new Set([['read', 'grant-seed', null], ['read', 'grant-journey-database', null]]),
    );
  }

  const revoked = change('revoke', 'other', 0);
  const repeatedRevoke = change('revoke', 'other', 0);
  assert.deepEqual(revoked.after.consumers, ['base', 'reader']);
  assert.deepEqual(repeatedRevoke.after, repeatedRevoke.before);
  change('revoke', 'reader', 0);
  const lastConsumer = change('revoke', 'base', 1);
  assert.deepEqual(lastConsumer.after, lastConsumer.before);

  for (const [transition, expected] of [
    [addedButRefused, true], [repeatedRefusal, false], [settledExisting, false],
    [repeatedSuccess, false], [addedAndSettled, true], [revoked, true], [repeatedRevoke, false],
  ]) {
    assert.equal(transition.result.declaration_changed, expected,
      `${transition.verb} ${transition.consumer}: declaration_changed contradicts persisted consumers`);
  }
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  const stopped = run('gpgconf', ['--homedir', keyring, '--kill', 'all']);
  if (stopped.status === 0) {
    rmSync(home, { recursive: true, force: true });
    rmSync(keyring, { recursive: true, force: true });
    rmSync(temporary, { recursive: true, force: true });
  } else {
    report.cleanup_error = 'The isolated GPG processes were not confirmed stopped; private fixture retained.';
    report.result = 'failed';
    process.exitCode = 1;
  }
  report.finished_at = new Date().toISOString();
  report.scope = 'Real grant and declaration transitions, persisted configuration and vault metadata; not database provisioning or GUI qualification.';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
