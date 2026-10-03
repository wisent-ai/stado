import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Destructive qualification against an already provisioned, dedicated SQLite
// fixture. All fixture paths must be inside this checkout's ignored .build.
// Required: STADO_DATABASE_TEST_HOME, STADO_DATABASE_TEST_CONFIG,
// STADO_DATABASE_TEST_KEYRING, STADO_DATABASE_TEST_NAME. No production defaults.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
function fixturePath(variable) {
  assert.ok(process.env[variable], `${variable} must name a dedicated test fixture`);
  const path = realpathSync(process.env[variable]);
  const within = relative(build, path);
  assert.ok(within && !isAbsolute(within) && within !== '..' && !within.startsWith('../'),
    `${variable} must be inside ${build}`);
  return path;
}
const home = fixturePath('STADO_DATABASE_TEST_HOME');
const config = fixturePath('STADO_DATABASE_TEST_CONFIG');
const keyring = fixturePath('STADO_DATABASE_TEST_KEYRING');
const name = process.env.STADO_DATABASE_TEST_NAME;
assert.ok(name, 'STADO_DATABASE_TEST_NAME must identify the provisioned test database');
const configuration = () => JSON.parse(readFileSync(config, 'utf8'));
const declaration = configuration().database_api?.databases?.[name];
assert.equal(declaration?.engine, 'sqlite', 'This journey only destroys a fleet SQLite fixture');
const item = declaration.item || `${name}-database`;
const vault = realpathSync(configuration().secrets.skarbiec.vault_file);
assert.ok(relative(home, vault) && !relative(home, vault).startsWith('..') && !isAbsolute(relative(home, vault)),
  'The owner vault must belong to the dedicated test home');
assert.equal(existsSync(join(home, '.stado', 'stado-skarbiec-token')), false,
  'This regression must run without a delegated Stado bearer');
const output = mkdtempSync(join(build, 'database-destroy-'));
const temporary = join(output, 'temporary');
mkdirSync(temporary, { mode: 0o700 });
const environment = {
  PATH: process.env.PATH, HOME: home, STADO_CONFIG: config, GNUPGHOME: keyring,
  TMPDIR: temporary, SKARBIEC_VAULT_FILE: vault,
  SKARBIEC_AUDIT_FILE: join(output, 'audit.jsonl'),
};
const stado = process.env.STADO_BIN || 'stado';
const skarbiec = process.env.SKARBIEC_BIN || 'skarbiec';
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
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
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/database', 'tests/database']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  for (const [key, binary, args] of [
    ['stado', stado, ['--version']], ['skarbiec', skarbiec, ['version']],
  ]) {
    const path = binary.includes('/') ? resolve(binary) : success(run('which', [binary]));
    report[key] = { path, sha256: digest(readFileSync(path)), version: success(run(binary, args)) };
  }
  environment.SKARBIEC_BIN = report.skarbiec.path;
  assert.equal(success(run(skarbiec, ['get', item, '--field', 'provider'])), 'fleet');
  const url = new URL(success(run(skarbiec, ['get', item, '--field', 'pooler_url'])));
  assert.equal(url.protocol, 'sqlite:');
  assert.equal(url.hostname, '', 'The fixture must use a local SQLite file');
  const data = realpathSync(decodeURIComponent(url.pathname));
  const expected = realpathSync(join(home, '.stado', 'databases', name, `${name}.sqlite3`));
  assert.equal(data, expected, 'The database must be the dedicated fixture, not an external file');
  assert.ok(statSync(data).isFile());
  const before = digest(readFileSync(data));
  report.before = { declaration, data, data_sha256: before };
  const destroyed = JSON.parse(success(run(stado, ['database', 'destroy', name, '--json'])));
  assert.equal(destroyed.provider, 'fleet');
  assert.equal(destroyed.destroyed, name);
  assert.equal(configuration().database_api?.databases?.[name], undefined);
  const items = JSON.parse(success(run(skarbiec, ['list'])));
  assert.ok(Array.isArray(items), 'Owner inventory must be a list');
  assert.equal(digest(readFileSync(data)), before, 'Destroy must retain the database data unchanged');
  assert.equal(items.some(entry => entry.id === item && entry.deleted !== true), false,
    'The credential item remains live in the owner vault');
  const absent = run(stado, ['database', 'destroy', name, '--json']);
  assert.equal(absent.status, 2, 'Destroy must refuse an undeclared database');
  report.after = { destroyed, declaration_absent: true, item_absent: true, data_sha256: before };
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  const stopped = run('gpgconf', ['--homedir', keyring, '--kill', 'all']);
  if (stopped.status !== 0) {
    report.result = 'failed';
    report.cleanup_error = 'The dedicated test keyring helpers were not confirmed stopped';
    process.exitCode = 1;
  } else {
    rmSync(temporary, { recursive: true, force: true });
  }
  report.finished_at = new Date().toISOString();
  report.scope = 'Real local-owner SQLite destruction, credential withdrawal and data retention; not provisioning or hosted-provider qualification';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
