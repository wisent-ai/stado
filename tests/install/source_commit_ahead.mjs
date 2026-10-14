import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado product install --source-commit` against the
// canonical checkout's own main. The Stado under test installs
// STADO_PRODUCT_TEST_PRODUCT's cli surface into an isolated home under .build
// from the commit at the head of its checkout's main (STADO_PRODUCT_TEST_SOURCE).
// That commit is canonical source whether or not origin/main carries it yet:
// the installation must finish, record the commit, and say on stderr that it
// installs ahead of origin exactly when origin/main does not carry it. A
// commit on neither main (written here as an unreferenced commit object, which
// moves no branch and touches no file of the checkout) must be refused naming
// both histories, leaving the receipt as it was. Required: STADO_BIN (the
// built Stado under test), STADO_PRODUCT_TEST_PRODUCT (a catalogued product
// with a cli surface) and STADO_PRODUCT_TEST_SOURCE (this machine's canonical
// checkout of it).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && product && source,
  'STADO_BIN, STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-source-commit-ahead-'));
const workspace = dirname(realpathSync(source));
const report = { started_at: new Date().toISOString(), product, source, workspace, cases: [], commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function run(program, args, home) {
  const env = home
    ? { PATH: process.env.PATH, HOME: home, STADO_CONFIG: join(home, 'no-config.toml'), TMPDIR: join(home, 'tmp') }
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

let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const home = join(output, 'home');
  mkdirSync(join(home, 'tmp'), { recursive: true });
  mkdirSync(join(home, '.stado', 'cache'), { recursive: true });
  const status = ['product', '--workspace', workspace, 'status', product, '--surface', 'cli', '--json'];
  const install = commit => ['product', '--workspace', workspace, 'install', product, '--surface', 'cli', '--source-commit', commit];

  // The head of the checkout's own main installs, ahead of origin or not.
  const head = success(run('git', ['-C', source, 'rev-parse', 'refs/heads/main']));
  success(run('git', ['-C', source, 'fetch', '--quiet', 'origin', 'main']));
  const onOrigin = !run('git', ['-C', source, 'merge-base', '--is-ancestor', head, 'origin/main']).status;
  report.main_head = { commit: head, on_origin: onOrigin };
  const installed = run(binary, install(head), home);
  success(installed);
  const after = JSON.parse(success(run(binary, status, home)));
  assert.equal(after.source_revision, head, 'the installation records the commit it built');
  const ahead = new RegExp(`${head} is on .*'s main but origin/main does not carry it yet`);
  assert.equal(ahead.test(installed.stderr), !onOrigin,
    'the installation says it is ahead of origin exactly when origin/main does not carry the commit');
  report.cases.push({ case: 'main-head', on_origin: onOrigin, status: after });

  // A commit on neither main is refused and changes no receipt.
  const tree = success(run('git', ['-C', source, 'rev-parse', 'HEAD^{tree}']));
  const stray = success(run('git', ['-C', source, 'commit-tree', tree, '-p', head, '-m', 'source-commit journey: a commit on no branch']));
  report.stray_commit = stray;
  const receipt = readFileSync(join(home, '.stado', 'products', product, 'cli.json'));
  const refused = run(binary, install(stray), home);
  assert.ok(refused.status && !refused.error, 'a commit on neither main must be refused');
  assert.match(refused.stderr, new RegExp(`neither origin/main nor .*'s main carries ${stray}; only canonical source is installed`));
  assert.deepEqual(readFileSync(join(home, '.stado', 'products', product, 'cli.json')), receipt,
    'a refused commit must leave the receipt as it was');
  report.cases.push({ case: 'stray-commit', refusal: refused.stderr });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'A cli source installation in an isolated home from the head of the canonical checkout\'s main and a refused commit on no branch; not a service installation, a forwarded host installation, product sync or a GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
