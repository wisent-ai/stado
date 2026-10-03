import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Destructive qualification of `stado service remove` against a dedicated,
// already-deployed launchd test unit whose logical directory route has a
// different name from the managed service it points at. Every fixture path
// must be inside this checkout's ignored .build. Required:
// STADO_SERVICE_TEST_HOME, STADO_SERVICE_TEST_CONFIG, STADO_SERVICE_TEST_HOST,
// STADO_SERVICE_TEST_UNIT. No production defaults.
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
const routesNaming = (service) => Object.entries(registry().service_directory?.services || {})
  .filter(([, route]) => route.active_host === host
    && [service.name, service.label, service.unit].filter(Boolean).includes(route.managed_service));
const output = mkdtempSync(join(build, 'service-remove-'));
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
function success(answer) {
  assert.equal(answer.status, 0, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/service', 'tests/service']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const service = record();
  assert.ok(service, `${unit} must be a managed service on ${host}`);
  assert.equal(service.kind, 'launchd', 'This journey only removes a launchd test unit');
  const unitFile = realpathSync(service.path);
  assert.ok(inside(unitFile, home), 'The unit file must belong to the dedicated test home');
  const routes = routesNaming(service);
  assert.ok(routes.some(([name]) => name !== service.name && name !== unit),
    'The fixture must carry a differently named logical route that names this service');
  report.before = { service, routes, unit_file: unitFile };

  const removed = JSON.parse(success(run(binary, ['service', 'remove', unit, '--host', host, '--json'])));
  assert.equal(removed.action, 'removed');
  assert.equal(removed.file.status === 'failed', false, removed.file.detail);
  assert.equal(record(), undefined, 'The managed record is still declared');
  assert.deepEqual(routesNaming(service), [], 'A directory route still names the removed service');
  assert.equal(existsSync(unitFile), false, 'The declared unit file is still on disk');
  const job = run('launchctl', ['print', `gui/${process.getuid()}/${unit}`]);
  assert.notEqual(job.status, 0, 'launchd still holds the removed job');

  const repeated = run(binary, ['service', 'remove', unit, '--host', host, '--json']);
  assert.notEqual(repeated.status, 0, 'Removing an undeclared service must be refused');
  report.after = { removed, record_absent: true, routes_absent: true, unit_file_absent: true };
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Real launchd removal with a differently named logical route; not systemd, privileged daemons or GUI qualification';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
