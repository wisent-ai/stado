import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado product <action> <product> --surface service
// --host <host>` for a registry host other than this machine: the operation is
// run by that host's own Stado over its host channel, not here. It reads the
// host's record of the service, asks the host to check an installation's
// arguments, has the host refuse an exact release coordinate for a service
// (the refusal comes back as `stado product … on <host> failed`, which only a
// run on the host produces), and confirms that `--catalog`, a file on this
// machine, is refused before the host is contacted. Nothing is built,
// installed or restarted. Required: STADO_PRODUCT_TEST_HOST (another registry
// host) and STADO_PRODUCT_TEST_PRODUCT (a catalogued product with a service
// surface). STADO_BIN selects the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const host = process.env.STADO_PRODUCT_TEST_HOST;
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
assert.ok(host && product, 'STADO_PRODUCT_TEST_HOST and STADO_PRODUCT_TEST_PRODUCT are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-remote-service-'));
const stado = process.env.STADO_BIN || 'stado';
const report = { started_at: new Date().toISOString(), host, product, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: process.env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.equal(answer.status, 0, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const escaped = text => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/setup', 'tests/product']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const service = ['--surface', 'service', '--host', host];

  const status = JSON.parse(success(run(binary, ['product', 'status', product, ...service, '--json'])));
  report.status = status;
  assert.equal(status.product, product, 'The host answered for another product');
  assert.equal(status.surface, 'service', 'The host answered for another surface');
  assert.equal(status.host, host, 'The record must be the one for the named host');

  const checked = JSON.parse(success(run(binary, ['product', 'install', product, ...service, '--check-arguments'])));
  assert.equal(checked.arguments, 'accepted', 'The host must accept the installation arguments');
  const unchanged = JSON.parse(success(run(binary, ['product', 'status', product, ...service, '--json'])));
  assert.equal(unchanged.installed_at, status.installed_at, '--check-arguments changed the installation on the host');

  const coordinate = run(binary, ['product', 'install', product, ...service,
    '--release-version', '0.0.0', '--source-commit', '0'.repeat(40)]);
  assert.notEqual(coordinate.status, 0, 'An exact release coordinate for a service must be refused');
  assert.match(coordinate.stderr,
    new RegExp(`stado product install ${escaped(product)} .* on ${escaped(host)} failed \\(exit \\d+\\): .*exact release coordinates require a local CLI`),
    'The refusal must come from the host, wrapped by the host channel');

  const catalog = join(root, 'catalog', 'products.yml');
  const refused = run(binary, ['product', 'install', product, ...service, '--catalog', catalog]);
  assert.notEqual(refused.status, 0, '--catalog for another host must be refused');
  assert.match(refused.stderr, /--catalog names a file on this machine/, 'The refusal must say why');
  assert.doesNotMatch(refused.stderr, new RegExp(`on ${escaped(host)} failed`), 'The host must not be contacted');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Remote service status, argument check, a host-side refusal and the catalog refusal on a real registry host; no build, installation or restart';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
