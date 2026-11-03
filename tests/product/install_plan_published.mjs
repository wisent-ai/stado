import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { pinnedGit } from '../native/home.mjs';
import { fileURLToPath } from 'node:url';

// Real qualification of what an installation plan reads in a workspace whose
// checkouts lag what is published. The journey builds a workspace under
// .build: a clone of this machine's Stado checkout whose `main` is reset
// behind its `origin/main`, and a checkout of STADO_PRODUCT_TEST_PRODUCT that
// holds only an older commit of that product's checkout, with that checkout
// as its origin. It runs `stado product install <product> --surface cli
// --source-commit <newest> --plan --json` with the Stado under test in an
// isolated home. Expected: the plan names the catalog it read as the Stado
// clone's origin/main, chosen `published-main`, never the lagging head; the
// product checkout now holds the newest commit, which the plan fetched from
// its origin before reading its release manifest; and a commit origin does
// not carry is refused naming the checkout. Required: STADO_BIN (the built
// Stado under test), STADO_PRODUCT_TEST_PRODUCT (a catalogued product with a
// cli surface), STADO_PRODUCT_TEST_SOURCE (this machine's checkout of it)
// and STADO_CATALOG_TEST_SOURCE (this machine's checkout of stado).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const catalogSource = process.env.STADO_CATALOG_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && product && source && catalogSource,
  'STADO_BIN, STADO_PRODUCT_TEST_PRODUCT, STADO_PRODUCT_TEST_SOURCE and STADO_CATALOG_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-install-plan-published-'));
const workspace = join(output, 'workspace');
const report = { started_at: new Date().toISOString(), product, source, catalog_source: catalogSource, workspace, cases: [], commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function run(program, args, home) {
  const env = home
    ? { ...pinnedGit, PATH: process.env.PATH, HOME: home, STADO_CONFIG: join(root, 'tests', 'config', 'empty.json'), TMPDIR: join(home, 'tmp') }
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
const git = (directory, ...args) => success(run('git', ['-C', directory, ...args]));

let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const home = join(output, 'home');
  mkdirSync(join(home, 'tmp'), { recursive: true });
  mkdirSync(join(home, '.stado', 'cache'), { recursive: true });
  mkdirSync(workspace, { recursive: true });

  // The catalog's checkout, its main three commits behind its origin/main,
  // with the canonical repository as its origin.
  const catalogCheckout = join(workspace, 'stado');
  success(run('git', ['clone', '--quiet', catalogSource, catalogCheckout]));
  git(catalogCheckout, 'remote', 'set-url', 'origin', git(catalogSource, 'remote', 'get-url', 'origin'));
  const published = git(catalogCheckout, 'rev-parse', 'origin/main');
  git(catalogCheckout, 'reset', '--quiet', '--hard', 'origin/main~3');
  const lagging = git(catalogCheckout, 'rev-parse', 'HEAD');
  report.catalog = { published, lagging };

  // The product's checkout holding only an older commit, read from this
  // machine's checkout (which this journey never writes); its origin, the
  // canonical repository, holds the newest.
  const canonical = git(source, 'remote', 'get-url', 'origin');
  const newest = success(run('git', ['ls-remote', '--exit-code', canonical, 'refs/heads/main'])).split(/\s/)[0];
  const older = git(source, 'rev-parse', 'origin/main~5');
  const productCheckout = join(workspace, product);
  success(run('git', ['init', '--quiet', '--initial-branch', 'main', productCheckout]));
  git(productCheckout, 'fetch', '--quiet', realpathSync(source), older);
  git(productCheckout, 'reset', '--quiet', '--hard', older);
  git(productCheckout, 'remote', 'add', 'origin', canonical);
  assert.ok(run('git', ['-C', productCheckout, 'cat-file', '-e', `${newest}^{commit}`]).status,
    'the product checkout must not hold the newest commit before the plan');
  report.product_checkout = { older, newest };

  const plan = commit => ['product', '--workspace', workspace, 'install', product, '--surface', 'cli',
    '--source-commit', commit, '--plan', '--json'];
  const planned = JSON.parse(success(run(binary, plan(newest), home)));
  assert.equal(planned.catalog?.chosen, 'published-main', 'the lagging catalog checkout is read at its origin/main');
  assert.equal(planned.catalog?.revision, published, 'the catalog revision is the published one');
  assert.notEqual(planned.catalog?.revision, lagging);
  assert.ok(!run('git', ['-C', productCheckout, 'cat-file', '-e', `${newest}^{commit}`]).status,
    'the plan fetched the source commit into the product checkout');
  report.cases.push({ case: 'lagging-checkouts', catalog: planned.catalog });

  // A commit id no repository holds is refused naming the checkout.
  const unknown = createHash('sha1').update(`plan journey ${output}`).digest('hex');
  assert.ok(run('git', ['-C', source, 'cat-file', '-e', `${unknown}^{commit}`]).status,
    'the refused commit id must name no commit of the product');
  const refused = run(binary, plan(unknown), home);
  assert.ok(refused.status && !refused.error, 'a commit origin does not carry must be refused');
  assert.match(refused.stderr, new RegExp(`${unknown} is not a commit of .*${product} even after fetching its origin`));
  report.cases.push({ case: 'unknown-commit', refusal: refused.stderr });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'An installation plan in an isolated home and workspace whose catalog checkout lags origin/main and whose product checkout lacks the source commit; not an installation that builds, product sync or a GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
