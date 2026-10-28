import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { hostname, networkInterfaces } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { connecting, hungSsh } from '../host_silence/hung_ssh.mjs';

// Real qualification that a local installation finishes while the registry
// authority accepts connections and answers nothing. Each case runs the
// Stado under test with HOME set to an isolated directory under .build. That
// home holds a receipt of a service surface recorded on another host, so the
// installation must decide whether that host is this machine, and a
// last-known-good registry copy naming, as the registry authority, a real
// OpenSSH server on a loopback port that runs the Stado under test
// (../host_silence/hung_ssh.mjs); it answers one registry read and is then
// stopped, so it accepts every connection and sends nothing.
// The installation of STADO_PRODUCT_TEST_PRODUCT's cli surface from
// STADO_PRODUCT_TEST_COMMIT (default the checkout's origin/main) must end on
// its own with the receipt recorded and no connection to the authority; with
// the copy removed it must refuse naming the copy, again without asking the
// authority. Required: STADO_BIN (the built Stado under test),
// STADO_PRODUCT_TEST_PRODUCT (a catalogued product with a cli surface) and
// STADO_PRODUCT_TEST_SOURCE (this machine's checkout of it). The journey
// holds no clock: an installation that waited on the authority never ends.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && product && source,
  'STADO_BIN, STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-registry-unreachable-'));
const workspace = dirname(realpathSync(source));
const report = { started_at: new Date().toISOString(), product, source, workspace, cases: [], commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
// The template's reader and the host this journey makes the hung authority.
const READER = 'marker-host';
const AUTHORITY = 'other-host';
// What the reader says when the authority answered its read with its own
// refusal: the live authority before it is stopped.
const AUTHORITY_ANSWERED = 'registry authority exited with';

function run(program, args, home) {
  const env = home
    ? { PATH: process.env.PATH, HOME: home, STADO_CONFIG: join(root, 'tests', 'config', 'empty.json'), TMPDIR: join(home, 'tmp') }
    : process.env;
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, home, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}

// This machine's IPv4 loopback, as the operating system lists it.
function loopback() {
  const found = Object.values(networkInterfaces()).flat()
    .find(entry => entry.internal && entry.family === 'IPv4');
  assert.ok(found, 'the operating system lists no IPv4 loopback interface');
  return found.address;
}

// The registry authority: a real SSH server on the loopback port Stado hands
// out now, recorded in `home`'s copy, which answers one registry read and is
// then stopped.
async function hungAuthority(binary, home) {
  const port = Number(success(run(binary, ['host', 'local', 'port'], home)));
  assert.ok(Number.isInteger(port), 'stado host local port prints a port');
  const authority = await hungSsh(home, loopback(), port);
  writeRecord(home, authority.destination, binary);
  const answered = run(binary, ['registry', 'self', '--name-only'], home);
  assert.ok(answered.stderr.includes(AUTHORITY_ANSWERED),
    `the live authority answers the read before it is stopped: ${answered.stderr}`);
  authority.hang();
  return authority;
}

function isolatedHome(name) {
  const home = join(output, name);
  mkdirSync(join(home, 'tmp'), { recursive: true });
  mkdirSync(join(home, '.stado', 'cache'), { recursive: true });
  mkdirSync(join(home, '.stado', 'products', 'example-remote'), { recursive: true });
  // A service surface this machine delivered to another host.
  writeFileSync(join(home, '.stado', 'products', 'example-remote', 'service.json'), JSON.stringify({
    product: 'example-remote', surface: 'service', status: 'installed', installed_at: '1970-01-01T00:00:00Z',
    recipe: {}, installed_paths: [], backups: [], host: AUTHORITY, source_revision: null, previous: null,
  }));
  return home;
}

// The local record: this machine as the template's reader, the served
// authority, running the Stado under test, as the registry authority.
function writeRecord(home, authority, binary) {
  const document = JSON.parse(readFileSync(join(root, 'tests', 'service_directory', 'registry.json'), 'utf8'));
  const reader = document.targets.find(target => target.name === READER);
  const remote = document.targets.find(target => target.name === AUTHORITY);
  assert.ok(reader && remote, `the template declares ${READER} and ${AUTHORITY}`);
  reader.hostnames = [hostname().toLowerCase()];
  reader.services = [];
  remote.ssh = authority;
  remote.services = [];
  document.service_directory.authority = { target: AUTHORITY, command: binary };
  document.service_directory.services = {};
  writeFileSync(join(home, '.stado', 'cache', 'registry-last-good.json'), JSON.stringify(document));
  writeFileSync(join(home, '.stado', 'cache', 'registry-last-good.meta.json'),
    JSON.stringify({ read_at: '1970-01-01T00:00:00Z', generation: 'example' }));
}

let failure;
let authority;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const commit = process.env.STADO_PRODUCT_TEST_COMMIT
    || success(run('git', ['-C', source, 'rev-parse', 'origin/main']));
  report.product_commit = commit;
  const install = ['product', '--workspace', workspace, 'install', product, '--surface', 'cli', '--source-commit', commit];
  const status = ['product', '--workspace', workspace, 'status', product, '--surface', 'cli', '--json'];

  // The local record names this machine: the installation finishes.
  const recorded = isolatedHome('recorded');
  authority = await hungAuthority(binary, recorded);
  report.authority = { destination: authority.destination, log: authority.log };
  const installed = run(binary, install, recorded);
  success(installed);
  const queried = run(binary, status, recorded);
  const after = JSON.parse(success(queried));
  assert.equal(after.source_revision, commit, 'the installation records the commit it built');
  for (const answer of [installed, queried]) {
    assert.ok(!answer.stderr.includes(connecting(authority.destination)),
      `the registry authority was asked during a local installation: ${answer.stderr}`);
  }
  report.cases.push({ case: 'recorded', status: after });

  // No local record: refused naming it, still without asking the authority.
  const unrecorded = isolatedHome('unrecorded');
  const refused = run(binary, install, unrecorded);
  assert.ok(refused.status && !refused.error, 'an installation that cannot tell this machine must refuse');
  assert.match(refused.stderr, new RegExp(`whether host ${AUTHORITY} is this machine cannot be told from this machine's own registry record`));
  assert.match(refused.stderr, /this machine keeps no usable local registry record: .*registry-last-good\.json/);
  assert.ok(!refused.stderr.includes(connecting(authority.destination)),
    `the registry authority was asked during a refused installation: ${refused.stderr}`);
  report.cases.push({ case: 'unrecorded', refusal: refused.stderr });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  authority?.close();
  report.finished_at = new Date().toISOString();
  report.scope = 'A cli source installation in isolated homes beside a receipt recorded on another host, against a registry authority that never answers; not a service installation on another host, product sync or a GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
