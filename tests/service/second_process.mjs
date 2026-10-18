import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { chmodSync, constants, mkdtempSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// A product runs as one process per host. A unit that would start a catalog
// product's own binary, a file named as the product, under any label but the
// product's one unit is refused before the host is touched, also for a
// product whose catalog program is a launcher rather than its binary. The
// registry must belong to an isolated home under this checkout's ignored
// build directory; nothing is deployed, since the refusal comes first.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
function fixture(variable) {
  const value = process.env[variable];
  assert.ok(value, `${variable} must name a dedicated test fixture`);
  const path = realpathSync(value);
  const within = relative(build, path);
  assert.ok(within && !isAbsolute(within) && !within.startsWith('..'),
    `${variable} must be inside ${build}`);
  return path;
}
const home = fixture('STADO_SERVICE_TEST_HOME');
const config = fixture('STADO_SERVICE_TEST_CONFIG');
const host = process.env.STADO_SERVICE_TEST_HOST;
const executable = process.env.STADO_BIN;
assert.ok(host && executable, 'STADO_SERVICE_TEST_HOST and STADO_BIN are required');

const output = mkdtempSync(join(build, 'service-second-process-'));
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { commands: [], result: 'failed' };
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(answer.status !== null && !answer.status,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const fileName = path => path.split('/').pop();

let failure;
try {
  const catalog = JSON.parse(success(run(executable, ['service', 'catalog', '--json'])));
  const launched = catalog.services.find(entry => fileName(entry.program) !== entry.name);
  assert.ok(launched, 'the catalog declares no product started through a launcher');
  const direct = catalog.services.find(entry => fileName(entry.program) === entry.name);
  assert.ok(direct, 'the catalog declares no product started as its own binary');
  for (const product of [launched, direct]) {
    const binary = join(output, product.name);
    writeFileSync(binary, '#!/bin/sh\n');
    chmodSync(binary, constants.S_IRWXU);
    const label = `com.wisent.test.second-${randomUUID()}`;
    const refused = run(executable,
      ['service', 'deploy', label, '--host', host, '--from', binary, '--launchd-label', label]);
    assert.notEqual(refused.status, null);
    assert.ok(refused.status, `a second ${product.name} process was admitted: ${refused.stdout}`);
    assert.match(refused.stderr, new RegExp(`a second ${product.name} process beside its one unit`),
      refused.stderr);
  }
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.scope = 'A unit starting a catalog product\'s own binary under another label, refused before any host change';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
