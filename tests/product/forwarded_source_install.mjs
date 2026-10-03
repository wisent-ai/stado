import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of a forwarded `stado product install <product>
// --surface service --host <host> --source-commit <sha>`: the host's own
// Stado installs the product from an exact commit, and the source travels as
// a `git archive` of this machine's checkout, which carries a pax global
// header naming the commit. The journey first has the host refuse a commit
// that does not exist, then installs the named commit and reads the host's
// record back: the installation is present and its source revision is the
// commit asked for. This installs a real product on a real host; point it at
// a host and product whose service may be replaced. Required:
// STADO_PRODUCT_TEST_HOST (a registry host other than this machine),
// STADO_PRODUCT_TEST_PRODUCT (a catalogued product with a service surface)
// and STADO_PRODUCT_TEST_SOURCE (this machine's checkout of that product).
// STADO_PRODUCT_TEST_COMMIT selects the commit, default the checkout's
// origin/main. STADO_BIN selects the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const host = process.env.STADO_PRODUCT_TEST_HOST;
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
assert.ok(host && product && source,
  'STADO_PRODUCT_TEST_HOST, STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-forwarded-install-'));
const stado = process.env.STADO_BIN || 'stado';
const report = { started_at: new Date().toISOString(), host, product, source, commands: [], result: 'failed' };
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
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/product', 'stado-rs/src/cli/setup', 'tests/product']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const commit = process.env.STADO_PRODUCT_TEST_COMMIT
    || success(run('git', ['-C', source, 'rev-parse', 'origin/main']));
  report.product_commit = commit;
  const service = ['--surface', 'service', '--host', host];
  report.before = JSON.parse(success(run(binary, ['product', 'status', product, ...service, '--json'])));

  const missing = run(binary, ['product', 'install', product, ...service, '--source-commit', 'f'.repeat(40)]);
  assert.notEqual(missing.status, 0, 'A commit that does not exist must be refused');
  assert.match(missing.stderr, new RegExp(`on ${escaped(host)} failed`), 'The refusal must come from the host');
  const unchanged = JSON.parse(success(run(binary, ['product', 'status', product, ...service, '--json'])));
  assert.equal(unchanged.source_revision, report.before.source_revision, 'A refused install changed the host record');

  success(run(binary, ['product', 'install', product, ...service, '--source-commit', commit]));
  const after = JSON.parse(success(run(binary, ['product', 'status', product, ...service, '--json'])));
  report.after = after;
  assert.equal(after.host, host, 'The record must be the one for the named host');
  assert.notEqual(after.status, 'absent', 'The host records no installation');
  assert.equal(after.source_revision, commit, 'The host installed another revision');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'One forwarded source install on a real registry host with a host-side refusal; not a release coordinate, rollback or GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
