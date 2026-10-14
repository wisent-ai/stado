import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { connect } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real source installation against the canonical Skarbiec checkout and a real
// Apple signing identity. Run only on a dedicated test account whose HOME
// contains that checkout and has no Skarbiec installation. The named broker
// endpoint must be stopped. This journey never cycles a service and retains
// its full receipts under the Stado checkout's .build.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = process.env.STADO_BIN;
const source = process.env.STADO_TEST_SKARBIEC_SOURCE;
const keychain = process.env.STADO_TEST_SIGNING_KEYCHAIN;
const identity = process.env.STADO_TEST_SIGNING_IDENTITY;
const stopped = process.env.STADO_TEST_STOPPED_SKARBIEC_URL;
const installationHome = process.env.STADO_TEST_INSTALL_HOME;
assert.ok(binary && source && keychain && identity && stopped && installationHome,
  'STADO_BIN, STADO_TEST_SKARBIEC_SOURCE, STADO_TEST_SIGNING_KEYCHAIN, STADO_TEST_SIGNING_IDENTITY, STADO_TEST_STOPPED_SKARBIEC_URL and STADO_TEST_INSTALL_HOME are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'skarbiec-signing-'));
const home = realpathSync(installationHome);
const catalogKeychain = join(home, 'Library', 'Keychains', 'login.keychain-db');
const stado = realpathSync(binary);
const checkout = realpathSync(source);
const workspace = dirname(checkout);
assert.ok(checkout.startsWith(`${home}/`), 'the canonical checkout must be below the dedicated test HOME');
assert.ok(!existsSync(catalogKeychain), 'the dedicated test HOME already holds the catalog signing keychain');
assert.ok(!existsSync(join(home, '.stado', 'products', 'skarbiec', 'cli.json')),
  'the dedicated test HOME already holds a Skarbiec installation');
assert.ok(!existsSync(join(home, '.stado', 'bin', 'skarbiec')),
  'the dedicated test HOME already holds a Skarbiec binary');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const report = {
  source_revision: null,
  stado: { path: stado, sha256: hash(readFileSync(stado)) },
  test_sha256: hash(readFileSync(fileURLToPath(import.meta.url))),
  commands: [], cases: [], result: 'failed',
};
const environment = { ...process.env, HOME: home, WC_SKARBIEC_URL: stopped };
for (const variable of ['WISENT_CODESIGN_ROLE', 'WISENT_CODESIGN_KEYCHAIN',
  'WISENT_CODESIGN_CERTIFICATE_PEM', 'WISENT_CODESIGN_PRIVATE_KEY_PEM']) {
  delete environment[variable];
}
environment.WISENT_CODESIGN_IDENTITY = identity;
function run(program, args, env = environment, cwd = root) {
  const answer = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function refusal(answer, cause) {
  assert.ok(answer.status && !answer.signal && !answer.error, cause);
  assert.match(answer.stderr, cause);
  return answer.stderr;
}
let failure;
let linked = false;
let installedByTest = false;
try {
  assert.equal(process.platform, 'darwin', 'a source install signs only on Darwin');
  const endpoint = new URL(stopped);
  assert.equal(endpoint.hostname, '127.0.0.1', 'the stopped Skarbiec endpoint must be isolated loopback');
  assert.equal(endpoint.protocol, 'http:', 'the stopped endpoint must use HTTP');
  assert.ok(endpoint.port, 'the stopped endpoint must declare its port');
  await new Promise((resolveStopped, reject) => {
    const socket = connect({ host: endpoint.hostname, port: Number(endpoint.port) });
    socket.once('connect', () => {
      socket.destroy();
      reject(new Error(`${stopped} answers connections; Skarbiec is not stopped`));
    });
    socket.once('error', error => {
      if (error.code === 'ECONNREFUSED') resolveStopped();
      else reject(error);
    });
  });
  report.stopped_skarbiec = stopped;
  const expected = realpathSync(keychain);
  report.source_revision = success(run('git', ['-C', checkout, 'rev-parse', 'origin/main']));
  const install = ['product', '--workspace', workspace, 'install', 'skarbiec',
    '--surface', 'cli', '--source-commit', report.source_revision, '--json'];
  const absent = refusal(run(stado, install), /WISENT_CODESIGN_KEYCHAIN names the keychain .*login\.keychain-db, which cannot be read/);
  assert.ok(!existsSync(join(home, '.stado', 'products', 'skarbiec', 'cli.json')),
    'an absent keychain changed the installed receipt');
  report.cases.push({ name: 'missing declared keychain', refusal: absent });

  mkdirSync(dirname(catalogKeychain), { recursive: true });
  symlinkSync(expected, catalogKeychain);
  linked = true;
  const installation = run(stado, install);
  if (!installation.status && !installation.signal && !installation.error) installedByTest = true;
  const installed = JSON.parse(success(installation));
  assert.equal(installed.status, 'installed');
  assert.equal(installed.source_revision, report.source_revision);
  assert.equal(installed.signing.source, 'local keychain');
  assert.equal(installed.signing.keychain, catalogKeychain);
  const placed = join(home, '.stado', 'bin', 'skarbiec');
  const actual = JSON.parse(success(run(stado, ['product', 'signing', 'inspect', placed, '--json'])));
  assert.equal(actual.state, 'stable');
  assert.equal(installed.signing.signatures[placed].identity, actual.authority);
  assert.equal(installed.signing.signatures[placed].team, actual.team);
  assert.equal(installed.signing.signatures[placed].cdhash, actual.cdhash);
  const status = JSON.parse(success(run(stado,
    ['product', '--workspace', workspace, 'status', 'skarbiec', '--surface', 'cli', '--json'])));
  assert.deepEqual(status.signing, installed.signing);
  report.cases.push({ name: 'source install with stopped Skarbiec', installed, status, actual });

  const receipt = readFileSync(join(home, '.stado', 'products', 'skarbiec', 'cli.json'));
  const wrong = refusal(run(stado, [...install, '--signing-keychain', expected]),
    /signing_keychain .* declares .* but the invocation names .*; no other signing source was tried/);
  assert.deepEqual(readFileSync(join(home, '.stado', 'products', 'skarbiec', 'cli.json')), receipt,
    'a conflicting signing source changed the installed receipt');
  report.cases.push({ name: 'conflicting signing source', refusal: wrong });
  const removed = JSON.parse(success(run(stado,
    ['product', '--workspace', workspace, 'remove', 'skarbiec', '--surface', 'cli', '--json'])));
  assert.equal(removed.status, 'absent');
  assert.equal(removed.signing, undefined, 'the absent receipt still reports a signer');
  assert.ok(!existsSync(placed), 'removal left the installed Skarbiec binary');
  report.cases.push({ name: 'removed isolated installation', removed });
  installedByTest = false;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  if (installedByTest) {
    const cleanup = run(stado,
      ['product', '--workspace', workspace, 'remove', 'skarbiec', '--surface', 'cli', '--json']);
    if (cleanup.status || cleanup.signal || cleanup.error) {
      report.cleanup_error = cleanup.stderr || cleanup.error?.message || cleanup.signal;
      failure ||= new Error(`removing test installation failed: ${report.cleanup_error}`);
      report.result = 'failed';
    }
  }
  if (linked) unlinkSync(catalogKeychain);
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
