import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--bearer-file' || !existsSync(args[1])) {
  console.error('usage: node tests/release_publishers/migration.mjs --bearer-file <existing-stado-bearer>');
  process.exit(2);
}
const bearer = resolve(args[1]);
const binary = process.env.STADO_BIN || 'stado';
const runs = join(root, '.build/real-tests/release-publishers');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
mkdirSync(home, { mode: 0o700 });
const config = join(home, '.stado', 'config.json');
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { started_at: new Date().toISOString(), binary, commands: [], result: 'failed' };

function run(program, arguments_, isolated = true) {
  const result = spawnSync(program, arguments_, {
    cwd: root, env: isolated ? environment : process.env, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024,
  });
  report.commands.push({ program, arguments: arguments_, isolated, exit_status: result.status,
    signal: result.signal, stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  return result;
}
function success(result) {
  assert.equal(result.status, 0, `${result.stderr || result.error || result.signal}`);
  return result.stdout.trim();
}

try {
  report.revision = success(run('git', ['rev-parse', 'HEAD'], false));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/release_catalog', 'stado-rs/src/cli/config_cmd/document/identities.rs'], false));
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  const executable = binary.includes('/') ? resolve(binary) : success(run('which', [binary], false));
  report.binary_sha256 = createHash('sha256').update(readFileSync(executable)).digest('hex');
  success(run(binary, ['config', 'init']));
  const initial = JSON.parse(readFileSync(config, 'utf8'));
  initial.secrets = { ...(initial.secrets || {}), skarbiec: { consumer: 'retired-consumer', token_file: bearer } };
  initial.release_api = { ...(initial.release_api || {}), publishers: {
    'example-product': { item: 'example-product-previous-publisher', prefix: 'example-product/' },
  } };
  const before = `${JSON.stringify(initial, null, 2)}\n`;
  writeFileSync(config, before, { mode: 0o600 });

  success(run(binary, ['config', 'migrate-identities']));
  const after = readFileSync(config, 'utf8');
  const migrated = JSON.parse(after);
  assert.equal(migrated.release_api.publishers['example-product'].item, 'example-product');
  assert.equal(migrated.release_api.publishers['example-product'].prefix, 'example-product/');
  assert.equal(migrated.secrets.skarbiec.consumer, 'stado');
  success(run(binary, ['config', 'validate']));
  const backups = readdirSync(dirname(config)).filter(name => name.startsWith('config.json.before-identity-migration-'));
  assert.equal(backups.length, 1);
  assert.equal(readFileSync(join(dirname(config), backups[0]), 'utf8'), before);
  success(run(binary, ['config', 'migrate-identities']));
  assert.equal(readFileSync(config, 'utf8'), after);
  assert.deepEqual(readdirSync(dirname(config)).filter(name => name.startsWith('config.json.before-identity-migration-')), backups);
  report.publisher = migrated.release_api.publishers['example-product'];
  report.backup_preserved = true;
  report.repeat_unchanged = true;

  const invalid = JSON.parse(after);
  invalid.release_api.publishers['example-product'].item = 'another-legacy-publisher';
  invalid.release_api.publishers['example-product'].prefix = 'another-product/';
  const invalidBytes = `${JSON.stringify(invalid, null, 2)}\n`;
  writeFileSync(config, invalidBytes, { mode: 0o600 });
  const refused = run(binary, ['config', 'migrate-identities']);
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, /prefix must be/);
  assert.equal(readFileSync(config, 'utf8'), invalidBytes);
  report.invalid_scope_refused_without_write = true;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack ?? String(error);
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Real CLI configuration migration and persisted file state; this does not qualify vault grants, publisher enrollment or mailbox reading.';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  rmSync(home, { recursive: true, force: true });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
