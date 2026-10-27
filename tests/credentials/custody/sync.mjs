// Real local-host custody through Stado and a genuine Skarbiec vault.
// Run: node tests/credentials/custody/sync.mjs
// Required: STADO_BIN, SKARBIEC_BIN, STADO_TOKEN_CUSTODY_LEGACY_BIN. The legacy
// executable must be a genuine native Stado release that supports config show
// but refuses credentials token custody. No executable or response is simulated.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, constants, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { hostname } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
function required(name) {
  const value = process.env[name]?.trim();
  assert.ok(value, `${name} is required for real custody qualification`);
  return value;
}
const stado = resolve(required('STADO_BIN'));
const skarbiec = resolve(required('SKARBIEC_BIN'));
const legacy = resolve(required('STADO_TOKEN_CUSTODY_LEGACY_BIN'));
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(root, '.build', 'credentials-custody-'));
chmodSync(output, constants.S_IRWXU);
const home = join(output, 'home');
// Keep GPG's socket path short, as in the other real credential journeys.
const keyring = mkdtempSync(join(root, '.build', 'g'));
const bin = join(home, '.stado', 'bin');
mkdirSync(bin, { recursive: true });
chmodSync(keyring, constants.S_IRWXU);
const installed = join(bin, 'stado');
const vault = join(home, 'vault.json');
const source = join(home, 'source.token');
const destination = join(home, 'destination.token');
const environment = {
  PATH: process.env.PATH, HOME: home, TMPDIR: output, GNUPGHOME: keyring,
  STADO_CONFIG: join(home, '.config', 'stado', 'config.json'),
  WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(home, 'objects'),
  SKARBIEC_VAULT_FILE: vault, SKARBIEC_AUDIT_FILE: join(home, 'audit.jsonl'),
  SKARBIEC_LAUNCHER: skarbiec, SKARBIEC_BIN: skarbiec,
};
const host = hostname().toLowerCase();
const consumer = `custody-reader-${randomUUID()}`;
const item = `custody-item-${randomUUID()}`;
const report = { commands: [], states: [], result: 'failed', started_at: new Date().toISOString() };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
let failure;
function run(program, args, { input, secret = false } = {}) {
  const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: 'utf8', input });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: secret ? undefined : answer.stdout, stderr: answer.stderr,
    stdout_contains_bearer: secret });
  assert.equal(answer.error, undefined, JSON.stringify(report.commands.at(-1)));
  assert.equal(answer.signal, null, JSON.stringify(report.commands.at(-1)));
  assert.notEqual(answer.status, null, JSON.stringify(report.commands.at(-1)));
  return answer;
}
function success(answer) {
  assert.ok(!answer.status, JSON.stringify({ status: answer.status, stderr: answer.stderr }));
  return answer.stdout;
}
function sync(json) {
  const args = ['credentials', 'token', 'sync', consumer, '--from-host', host,
    '--host', host, '--source-token-file', source, '--token-file', destination, '--check'];
  if (json) args.push('--json');
  return run(stado, args);
}
function unchanged(expected) {
  const observed = { source: digest(readFileSync(source)), vault: digest(readFileSync(vault)) };
  assert.throws(() => readFileSync(destination), { code: 'ENOENT' });
  report.states.push({ ...observed, destination_exists: false });
  assert.deepEqual(observed, expected, 'a refused check changed the bearer or vault');
}
function refusal(answer, operation, file, reference, json, bearer) {
  assert.ok(answer.status, 'the refused host operation was reported as successful');
  const document = json ? JSON.parse(answer.stdout) : undefined;
  if (document) {
    assert.equal(document.status, 'error');
    assert.equal(document.error_code, 'refused');
  }
  const diagnostic = json ? document.message : answer.stderr;
  assert.equal(typeof diagnostic, 'string', 'the refusal has no diagnostic message');
  assert.ok(diagnostic.includes(`${host}: credentials token custody ${operation}`), diagnostic);
  assert.ok(diagnostic.includes(`for ${consumer} at ${file} (exit ${reference.status})`), diagnostic);
  assert.ok(!diagnostic.includes(bearer), 'the refusal disclosed the export bearer');
  return diagnostic;
}
try {
  const revision = success(run('git', ['rev-parse', 'HEAD'])).trim();
  const patch = success(run('git', ['diff', '--no-ext-diff', '--no-textconv', '--binary', 'HEAD']));
  writeFileSync(join(output, 'source.patch'), patch);
  report.source = { revision, patch_sha256: digest(patch), journey_sha256: digest(readFileSync(fileURLToPath(import.meta.url))) };
  for (const [name, binary] of Object.entries({ stado, skarbiec, legacy })) {
    report[name] = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])).trim() };
  }
  assert.ok(report.stado.version.split(/\s+/).some(part => part.replace(/[()]/g, '') === revision),
    'the executable does not identify this source revision');
  assert.equal(patch, '', 'qualify committed source, not an unrelated dirty checkout');
  success(run(skarbiec, ['init', `custody-owner-${randomUUID()}`]));
  success(run(skarbiec, ['set', item, '--type', 'api-key'], {
    input: JSON.stringify({ schema: 'skarbiec.item.v2', kind: 'api-key', fields: { api_key: randomUUID() }, context: {} }),
  }));
  const issued = JSON.parse(success(run(skarbiec, ['grant', 'issue', consumer,
    '--capabilities', `read:${item}#api_key`, '--until-revoked'], { secret: true })));
  for (const path of [source, destination]) {
    writeFileSync(path, issued.token, { mode: constants.S_IRUSR | constants.S_IWUSR });
  }
  copyFileSync(stado, installed);
  success(run(stado, ['config', 'init']));
  success(run(stado, ['config', 'set', 'secrets.skarbiec.vault_file', vault]));
  const registry = JSON.parse(readFileSync(join(root, 'tests/service_directory/registry.json'), 'utf8'));
  const [target] = registry.targets;
  target.name = host;
  target.hostnames = [host];
  // Node spells the release protocol's amd64 architecture x64.
  target.release_platform = `${process.platform}-${process.arch === 'x64' ? 'amd64' : process.arch}`;
  registry.targets = [target];
  registry.service_directory.authority = { target: host, command: stado };
  registry.service_directory.services = {};
  const registryFile = join(output, 'registry.json');
  writeFileSync(registryFile, JSON.stringify(registry));
  success(run(stado, ['registry', 'import', registryFile]));
  const checked = JSON.parse(success(sync(true)));
  assert.equal(checked.status, 'token_checked');
  assert.equal(digest(readFileSync(source)), digest(readFileSync(destination)));
  const granted = JSON.parse(success(run(skarbiec, ['grant', 'verify', consumer, item,
    '--action', 'read', '--field', 'api_key', '--token-file', destination])));
  assert.equal(granted.allowed, true, 'the checked bearer does not authorize its real consumer');
  rmSync(destination);
  const expected = { source: digest(readFileSync(source)), vault: digest(readFileSync(vault)) };
  const exported = run(stado, ['credentials', 'token', 'custody', 'export', vault, consumer, source], { secret: true });
  success(exported);
  const failedCheck = run(stado, ['credentials', 'token', 'custody', 'check', vault, consumer, destination], { input: exported.stdout });
  assert.ok(failedCheck.status, 'the reference check accepted a missing bearer');
  for (const json of [false, true]) {
    const diagnostic = refusal(sync(json), 'check', destination, failedCheck, json, issued.token);
    assert.ok(diagnostic.includes('destination token file is missing or does not match the declared consumer grant'), diagnostic);
    unchanged(expected);
  }
  // Only this isolated home changes; no operator installation or service is touched.
  copyFileSync(legacy, installed);
  const failedExport = run(installed, ['credentials', 'token', 'custody', 'export', vault, consumer, source], { secret: true });
  assert.ok(failedExport.status, 'the legacy fixture unexpectedly supports custody');
  assert.ok(failedExport.stderr.includes('\nUsage:'), 'the real legacy executable must supply a multiline usage refusal');
  for (const json of [false, true]) {
    const diagnostic = refusal(sync(json), 'export', source, failedExport, json, issued.token);
    assert.ok(diagnostic.includes(`stderr:\n${failedExport.stderr.trim()}`), diagnostic);
    unchanged(expected);
  }
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = String(error.stack);
} finally {
  try {
    success(run('gpgconf', ['--homedir', keyring, '--kill', 'all']));
    rmSync(home, { recursive: true, force: true });
    rmSync(keyring, { recursive: true, force: true });
  } catch (error) {
    report.result = 'failed';
    report.cleanup_error = String(error.stack);
    failure = new AggregateError([failure, error], 'custody qualification cleanup failed');
  }
  report.finished_at = new Date().toISOString();
  report.scope = 'Real local-host check, Skarbiec authorization, missing-destination and genuine legacy-export refusals in CLI text and JSON; no GUI qualification';
  writeFileSync(join(output, 'report.json'), JSON.stringify(report), { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(join(output, 'report.json'));
}
if (failure) throw failure;
