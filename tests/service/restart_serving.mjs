import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Qualification of `stado service restart` answering on what the restarted
// unit does with its declared port, against a dedicated, already-deployed
// launchd test unit whose service directory route declares one loopback
// endpoint on this host. Case one: the unit serves its port, so the restart
// answers serving and succeeds. Case two: this journey holds the declared
// address itself before the restart, so the unit cannot bind it; the restart
// must fail with not_serving and name the port. Case three: with the address
// free again the unit serves once more. Every fixture path must be inside this
// checkout's ignored .build. Required: STADO_SERVICE_TEST_HOME,
// STADO_SERVICE_TEST_CONFIG, STADO_SERVICE_TEST_HOST, STADO_SERVICE_TEST_UNIT.
// No production defaults. A failed case is rethrown after the report is
// written, so the process exits unsuccessfully.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
function inside(path, parent) {
  const within = relative(parent, path);
  return Boolean(within) && !isAbsolute(within) && !within.startsWith('..');
}
function fixturePath(variable) {
  assert.ok(process.env[variable], `${variable} must name a dedicated test fixture`);
  const path = realpathSync(process.env[variable]);
  assert.ok(inside(path, build), `${variable} must be inside ${build}`);
  return path;
}
const home = fixturePath('STADO_SERVICE_TEST_HOME');
const config = fixturePath('STADO_SERVICE_TEST_CONFIG');
const host = process.env.STADO_SERVICE_TEST_HOST;
const unit = process.env.STADO_SERVICE_TEST_UNIT;
assert.ok(host && unit, 'STADO_SERVICE_TEST_HOST and STADO_SERVICE_TEST_UNIT are required');
const registryFile = join(home, '.stado', 'local-storage', 'registry.json');
const registry = () => JSON.parse(readFileSync(registryFile, 'utf8'));
const target = () => registry().targets.find(entry => entry.name === host);
const record = () => (target()?.services || []).find(entry => entry.label === unit || entry.unit === unit);
// The one endpoint the service directory declares for this unit on this host.
const declaredEndpoint = (service) => {
  const routes = Object.values(registry().service_directory?.services || {})
    .filter(route => route.active_host === host
      && [service.name, service.label, service.unit].filter(Boolean).includes(route.managed_service));
  const [declared, ...others] = [...new Set(routes.flatMap(route =>
    Object.values(route.endpoints || {}).map(endpoint => endpoint.url)))];
  assert.ok(declared && !others.length, `${unit} must declare exactly one endpoint on ${host}`);
  const url = new URL(declared);
  assert.ok(url.port, `${declared} names no port`);
  return { hostname: url.hostname, port: Number(url.port) };
};
const output = mkdtempSync(join(build, 'service-restart-'));
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const stado = process.env.STADO_BIN || 'stado';
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const succeeded = answer => answer.status !== null && !answer.status;
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function hold({ hostname, port }) {
  return new Promise((accept, refuse) => {
    const server = createServer();
    server.once('error', refuse);
    server.listen(port, hostname, () => accept(server));
  });
}
const release = server => new Promise(done => server.close(done));
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/service', 'tests/service']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const service = record();
  assert.ok(service, `${unit} must be a managed service on ${host}`);
  assert.equal(service.kind, 'launchd', 'This journey restarts a launchd test unit');
  assert.ok(inside(realpathSync(service.path), home), 'The unit file must belong to the dedicated test home');
  const endpoint = declaredEndpoint(service);
  report.before = { service, endpoint };
  const restart = () => run(binary, ['service', 'restart', unit, '--host', host, '--json']);
  const allSay = (entries, word) => entries.length && entries.every(entry => entry.serving === word);

  const served = JSON.parse(success(restart()));
  assert.ok(allSay(served, 'serving'), `${unit} does not serve port ${endpoint.port} after the restart`);
  report.serving = served;

  success(run(binary, ['service', 'stop', unit, '--host', host, '--json']));
  const holder = await hold(endpoint);
  try {
    const refused = restart();
    assert.ok(!succeeded(refused), 'A restart whose unit cannot bind its port must fail');
    const answer = JSON.parse(refused.stdout || '[]');
    report.taken = answer;
    assert.ok(allSay(answer, 'not_serving'), 'The unit is reported serving a port it cannot hold');
    const said = `${refused.stderr}\n${answer.map(entry => entry.serving_detail || '').join('\n')}`;
    assert.match(said, new RegExp(`\\b${endpoint.port}\\b`), 'The refusal must name the declared port');
  } finally {
    await release(holder);
  }
  const recovered = JSON.parse(success(restart()));
  assert.ok(allSay(recovered, 'serving'), 'The unit must serve again once its port is free');
  report.after = recovered;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Real launchd restart of a dedicated test unit with one declared endpoint; not systemd or privileged daemons';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) {
  throw failure;
}
