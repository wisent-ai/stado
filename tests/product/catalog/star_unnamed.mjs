import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification that one product without a star_repository does not
// take the catalog with it: the journey runs the Stado under test with HOME
// and the workspace isolated under .build, writes Stado's own committed
// catalog beside no checkout with the first product's star_repository
// removed, and reads it. Expected: `stado product catalog --json` answers
// every product, the first without a star_repository; a star_repository
// that is not OWNER/REPO is still refused naming the product and field.
// The star request's own report (`documentation star-repositories`) asks
// GitHub and is not run here.
// Required: STADO_BIN, the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: the Stado executable under test');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-catalog-star-unnamed-'));
const home = join(output, 'home');
const workspace = join(home, 'workspace');
const catalogPath = join(output, 'loose', 'products.yml');
const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: workspace, GIT_CONFIG_GLOBAL: '/dev/null' };
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const succeeded = answer => answer.status !== null && !answer.status;
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: isolated, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout;
}
const read = () => run(stado, ['product', '--workspace', workspace, '--catalog', catalogPath, 'catalog', '--json']);
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD'])).trim();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])).trim() };

  const committed = success(run('git', ['-C', root, 'show', 'HEAD:catalog/products.yml']));
  const lines = committed.split('\n');
  const first = lines.findIndex(line => line.startsWith('- id: '));
  const next = lines.findIndex((line, index) => index > first && line.startsWith('- id: '));
  assert.ok(first >= 0 && next > first, 'the committed catalog holds more than one product');
  const unnamed = lines[first].slice('- id: '.length);
  const star = lines.findIndex((line, index) => index > first && index < next && line.startsWith('  star_repository:'));
  assert.ok(star > first, `${unnamed} declares a star_repository in the committed catalog`);
  const total = lines.filter(line => line.startsWith('- id: ')).length;
  mkdirSync(dirname(catalogPath), { recursive: true });

  // One product without a star_repository: the whole catalog is answered.
  writeFileSync(catalogPath, lines.filter((_, index) => index !== star).join('\n'));
  const answered = read();
  success(answered);
  const products = JSON.parse(answered.stdout).products;
  assert.equal(products.length, total, 'a product without a star_repository took other products with it');
  const record = products.find(product => product.id === unnamed);
  assert.ok(record, `${unnamed} was dropped from the answer`);
  assert.equal(record.star_repository, undefined, `${unnamed} was answered with a star_repository nobody declared`);
  report.unnamed = unnamed;

  // A star_repository that is declared but is not OWNER/REPO is refused.
  const malformed = [...lines];
  malformed[star] = '  star_repository: not-a-repository';
  writeFileSync(catalogPath, malformed.join('\n'));
  const refused = read();
  assert.ok(!succeeded(refused), 'a star_repository that is not OWNER/REPO was answered');
  assert.ok(refused.stderr.includes(`${unnamed}.star_repository`), `the refusal does not name the field: ${refused.stderr}`);
  assert.match(refused.stderr, /expected OWNER\/REPO/);
  report.refusal = refused.stderr;
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  rmSync(home, { recursive: true, force: true });
}
if (failure) throw failure;
