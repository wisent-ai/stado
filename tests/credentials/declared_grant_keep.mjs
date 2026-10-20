import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { hostname } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado credentials grant renew` keeping every
// declared consumer grant until revoked, through the built Stado and a real
// Skarbiec owner vault of its own in an isolated home under .build/ (HOME,
// STADO_CONFIG, SKARBIEC_VAULT_FILE, SKARBIEC_AUDIT_FILE and GNUPGHOME all
// point there, so the operator's vault, keyring and registry are never read
// or written).
//
// The vault holds the agent's own grant, already until revoked, and two
// direct grants issued with an end: one a consumer the imported registry's
// service directory declares, one that no declaration owns. One renew:
// - keeps the declared grant until revoked, and the bearer it held before
//   still verifies;
// - leaves the undeclared grant with its end and names it;
// - a second renew keeps nothing more.
// Refusal: with the vault's grants unreadable (SKARBIEC_VAULT_FILE names a
// file that is not a vault), renew exits non-zero naming the failed read.
//
// The registry is the service directory journeys' document
// (tests/service_directory/registry.json) placed on this machine, with one
// service whose consumer declares the grant.
//
// Required: STADO_BIN (the Stado under test), SKARBIEC_BIN (the Skarbiec
// under test, one that has `grant keep`) and SKARBIEC_TEST_GRANT_TTL (the
// lifetime in whole seconds of the grants this test issues with an end).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value, `${name} must be set; the header of ${fileURLToPath(import.meta.url)} says what it names`);
  return value;
};
const stado = resolve(required('STADO_BIN'));
const skarbiec = resolve(required('SKARBIEC_BIN'));
const ttl = required('SKARBIEC_TEST_GRANT_TTL');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-declared-keep-'));
const home = join(output, 'home');
// gpg-agent's socket lives in the keyring and a Unix socket path is short on
// macOS, so the keyring sits at a short path of its own.
const keyring = mkdtempSync(join(realpathSync(join(root, '.build')), 'g'));
mkdirSync(join(home, '.stado'), { recursive: true });
chmodSync(keyring, constants.S_IRWXU);
chmodSync(output, constants.S_IRWXU);
const vault = join(home, 'vault.json');
const environment = {
  PATH: process.env.PATH,
  HOME: home,
  TMPDIR: output,
  GNUPGHOME: keyring,
  STADO_CONFIG: join(home, '.stado', 'config.json'),
  SKARBIEC_VAULT_FILE: vault,
  SKARBIEC_AUDIT_FILE: join(home, 'audit.jsonl'),
  SKARBIEC_LAUNCHER: skarbiec,
  SKARBIEC_BIN: skarbiec,
};
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const succeeded = answer => answer.status !== null && !answer.status;
// Every command and its streams go to the report, except a bearer: a command
// that prints one is run with `secret` and its stdout is not kept.
function run(program, args, { env = environment, secret = false, input } = {}) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8', input });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: secret ? '(a bearer; not kept)' : answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function listed(consumer) {
  const grants = JSON.parse(success(run(skarbiec, ['grant', 'list'])));
  const row = grants.find(grant => grant.consumer === consumer);
  assert.ok(row, `skarbiec grant list does not list ${consumer}`);
  return row;
}
const suffix = `${process.pid}-${Date.now()}`;
const item = `declared-keep-item-${suffix}`;
const declared = `declared-keep-reader-${suffix}`;
const undeclared = `undeclared-keep-reader-${suffix}`;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD'], { env: process.env }));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: stado, sha256: digest(readFileSync(stado)), version: success(run(stado, ['--version'])) };
  report.skarbiec = { path: skarbiec, sha256: digest(readFileSync(skarbiec)) };

  success(run(skarbiec, ['init', `declared-keep-owner-${suffix}`]));
  success(run(skarbiec, ['set-json', item, '--type', 'api-key'], {
    input: JSON.stringify({ schema: 'skarbiec.item.v2', kind: 'api-key', fields: { api_key: 'not-a-secret' }, context: {} }),
  }));
  // The agent's own grant already lives until revoked, so renew has nothing
  // to do for it; its bearer is the file the agent configuration names.
  const agent = JSON.parse(success(run(skarbiec, ['grant', 'issue', 'stado', '--capabilities',
    `read:${item}#api_key`, '--until-revoked'], { secret: true })));
  const agentToken = join(home, '.stado', 'workload-agent-skarbiec-token');
  writeFileSync(agentToken, agent.token, { mode: constants.S_IRUSR | constants.S_IWUSR });
  const issued = JSON.parse(success(run(skarbiec, ['grant', 'issue', declared, '--capabilities',
    `read:${item}#api_key`, '--ttl-seconds', ttl], { secret: true })));
  const bearer = join(output, 'declared.token');
  writeFileSync(bearer, issued.token, { mode: constants.S_IRUSR | constants.S_IWUSR });
  success(run(skarbiec, ['grant', 'issue', undeclared, '--capabilities', `read:${item}#api_key`,
    '--ttl-seconds', ttl], { secret: true }));
  assert.equal(listed(declared).until_revoked, false, 'the declared grant was issued without an end');

  success(run(stado, ['config', 'init']));
  const host = hostname().toLowerCase();
  const registry = JSON.parse(readFileSync(join(root, 'tests/service_directory/registry.json'), 'utf8'));
  const [placed] = registry.targets;
  placed.name = host;
  placed.hostnames = [host];
  registry.targets = [placed];
  registry.service_directory.authority = { target: host, command: stado };
  registry.service_directory.services = {
    'keep-api': {
      active_host: host,
      endpoints: {},
      consumers: {
        'keep-product': {
          grants: [{ consumer: declared, capabilities: [`read:${item}#api_key`], token_file: bearer }],
        },
      },
    },
  };
  const registryFile = join(output, 'registry.json');
  writeFileSync(registryFile, `${JSON.stringify(registry, null, 2)}\n`);
  success(run(stado, ['registry', 'import', registryFile]));

  const renewed = JSON.parse(success(run(stado, ['credentials', 'grant', 'renew', '--json'])));
  report.renewed = renewed;
  assert.equal(renewed.failed, false, `renew failed: ${JSON.stringify(renewed.lines)}`);
  assert.ok(renewed.lines.some(line => line.includes(`kept ${declared} until revoked`)),
    `renew did not say it kept ${declared}: ${JSON.stringify(renewed.lines)}`);
  assert.ok(renewed.lines.some(line => line.includes(`${undeclared} has an end and no service directory declaration owns it`)),
    `renew did not name ${undeclared}: ${JSON.stringify(renewed.lines)}`);
  assert.equal(listed(declared).until_revoked, true, 'the declared grant does not live until revoked');
  assert.equal(listed(undeclared).until_revoked, false, 'the undeclared grant lost its end');
  const verified = JSON.parse(success(run(skarbiec, ['grant', 'verify', declared, item, '--action', 'read',
    '--field', 'api_key', '--token-file', bearer])));
  assert.equal(verified.allowed, true, 'the bearer the declared consumer held no longer verifies');

  const again = JSON.parse(success(run(stado, ['credentials', 'grant', 'renew', '--json'])));
  assert.ok(!again.lines.some(line => line.includes(`kept ${declared}`)), 'a second renew kept the declared grant again');

  const notVault = join(output, 'not-a-vault.json');
  writeFileSync(notVault, '{}\n');
  const refused = run(stado, ['credentials', 'grant', 'renew', '--json'],
    { env: { ...environment, SKARBIEC_VAULT_FILE: notVault } });
  assert.ok(!succeeded(refused), `renew over a file that is not a vault passed: ${refused.stdout}`);
  assert.ok(`${refused.stdout}${refused.stderr}`.includes('declared grants: '),
    `the refusal does not say why the declared pass could not read the vault: ${refused.stdout} ${refused.stderr}`);
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
} finally {
  spawnSync('gpgconf', ['--homedir', keyring, '--kill', 'all']);
  rmSync(keyring, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  report.scope = 'stado credentials grant renew keeps a declared consumer grant until revoked with its bearer, leaves an undeclared one with its end, and refuses an unreadable grant listing';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (report.error) {
  throw new Error(`declared grant keep qualification failed; report in ${output}: ${report.error}`);
}
