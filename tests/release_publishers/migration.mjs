import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const usage = 'usage: STADO_BIN=<stado> node tests/release_publishers/migration.mjs --bearer-file <existing-stado-bearer>';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const [, , flag, bearerPath, ...extra] = process.argv;
if (flag !== '--bearer-file' || !bearerPath || extra.length || !existsSync(bearerPath)) {
  throw new Error(usage);
}
const binary = process.env.STADO_BIN;
if (!binary) {
  throw new Error(`STADO_BIN is not set: name the stado binary this test runs. ${usage}`);
}
const bearer = resolve(bearerPath);
const ownerOnlyDirectory = constants.S_IRWXU;
const ownerOnlyFile = constants.S_IRUSR | constants.S_IWUSR;
const runs = join(root, '.build/real-tests/release-publishers');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
mkdirSync(home, { mode: ownerOnlyDirectory });
const config = join(home, '.stado', 'config.json');
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { started_at: new Date().toISOString(), binary, commands: [], result: 'failed' };
const backupPrefix = 'config.json.before-identity-migration-';

function run(program, arguments_, isolated = true) {
  const result = spawnSync(program, arguments_, {
    cwd: root, env: isolated ? environment : process.env, encoding: 'utf8',
  });
  report.commands.push({ program, arguments: arguments_, isolated, exit_status: result.status,
    signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  return result;
}
function success(result) {
  const cause = [result.stderr, result.error?.message, result.signal].filter(Boolean).join('; ');
  assert.equal(result.status, 0, cause);
  return result.stdout.trim();
}
function backups() {
  return readdirSync(dirname(config)).filter(name => name.startsWith(backupPrefix));
}

let failure;
try {
  report.revision = success(run('git', ['rev-parse', 'HEAD'], false));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/release_catalog', 'stado-rs/src/cli/config_cmd/document/identities.rs'], false));
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  const executable = binary.includes('/') ? resolve(binary) : success(run('which', [binary], false));
  report.binary_sha256 = createHash('sha256').update(readFileSync(executable)).digest('hex');
  success(run(binary, ['config', 'init']));
  const initial = JSON.parse(readFileSync(config, 'utf8'));
  initial.secrets = { ...initial.secrets, skarbiec: { consumer: 'retired-consumer', token_file: bearer } };
  initial.release_api = { ...initial.release_api, publishers: {
    'example-product': { item: 'example-product-previous-publisher', prefix: 'example-product/' },
  } };
  const before = `${JSON.stringify(initial, null, 2)}\n`;
  writeFileSync(config, before, { mode: ownerOnlyFile });

  success(run(binary, ['config', 'migrate']));
  const after = readFileSync(config, 'utf8');
  const migrated = JSON.parse(after);
  assert.equal(migrated.release_api.publishers['example-product'].item, 'example-product');
  assert.equal(migrated.release_api.publishers['example-product'].prefix, 'example-product/');
  assert.equal(migrated.secrets.skarbiec.consumer, 'stado');
  success(run(binary, ['config', 'validate']));
  const [backup, ...otherBackups] = backups();
  assert.ok(backup, 'the migration kept no copy of the previous file');
  assert.deepEqual(otherBackups, [], 'the migration kept more than one copy of the previous file');
  assert.equal(readFileSync(join(dirname(config), backup), 'utf8'), before);
  success(run(binary, ['config', 'migrate']));
  assert.equal(readFileSync(config, 'utf8'), after);
  assert.deepEqual(backups(), [backup]);
  report.publisher = migrated.release_api.publishers['example-product'];
  report.backup_preserved = true;
  report.repeat_unchanged = true;

  const invalid = JSON.parse(after);
  invalid.release_api.publishers['example-product'].item = 'another-legacy-publisher';
  invalid.release_api.publishers['example-product'].prefix = 'another-product/';
  const invalidBytes = `${JSON.stringify(invalid, null, 2)}\n`;
  writeFileSync(config, invalidBytes, { mode: ownerOnlyFile });
  const refused = run(binary, ['config', 'migrate']);
  assert.ok(refused.status, `the migration accepted a publisher outside its product: ${refused.stdout}`);
  assert.match(refused.stderr, /prefix must be/);
  assert.equal(readFileSync(config, 'utf8'), invalidBytes);
  report.invalid_scope_refused_without_write = true;
  report.result = 'passed';
} catch (error) {
  report.error = error instanceof Error ? error.stack : String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Real CLI configuration migration and persisted file state; this does not qualify vault grants, publisher enrollment or mailbox reading.';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: ownerOnlyFile });
  rmSync(home, { recursive: true, force: true });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) {
  throw failure;
}
