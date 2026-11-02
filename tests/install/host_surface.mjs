import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `--host` on a surface that installs on the machine
// running the command. The Stado under test is asked, in an isolated home
// under .build, to install and to update STADO_PRODUCT_TEST_PRODUCT's cli
// surface with `--host`, and to reconcile the cli surface with `--host`. Each
// must be refused before anything is built, naming that --host would be
// ignored and how to reach the named host, and must leave no installation
// receipt behind; the same installation without --host is not exercised here
// (tests/install/source_commit_ahead.mjs installs a cli). Required: STADO_BIN
// (the built Stado under test), STADO_PRODUCT_TEST_PRODUCT (a catalogued
// product with a cli surface) and STADO_PRODUCT_TEST_SOURCE (this machine's
// canonical checkout of it).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && product && source,
  'STADO_BIN, STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-host-surface-'));
const workspace = dirname(realpathSync(source));
const named = 'another-host';
const report = { started_at: new Date().toISOString(), product, source, workspace, named, cases: [], commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

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

let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const home = join(output, 'home');
  mkdirSync(join(home, 'tmp'), { recursive: true });
  mkdirSync(join(home, '.stado', 'cache'), { recursive: true });
  const receipt = join(home, '.stado', 'products', product, 'cli.json');
  const builds = join(home, '.stado', 'products', 'builds');
  const head = success(run('git', ['-C', source, 'rev-parse', 'refs/heads/main']));

  for (const action of ['install', 'update']) {
    const refused = run(binary, ['product', '--workspace', workspace, action, product, '--surface', 'cli',
      '--host', named, '--source-commit', head], home);
    assert.ok(refused.status && !refused.error && !refused.signal, `${action} --surface cli --host must be refused`);
    assert.match(refused.stderr, new RegExp(
      `--host names the host a service surface's unit runs on; the cli surface installs on the machine that runs this command, so --host ${named} would be ignored`));
    assert.match(refused.stderr, new RegExp(`run the installation on ${named}`));
    assert.match(refused.stderr, /Nothing was built or installed/);
    assert.ok(!existsSync(receipt), `a refused ${action} must leave no installation receipt`);
    assert.ok(!existsSync(builds), `a refused ${action} must build nothing`);
    report.cases.push({ case: `${action}-cli-host`, refusal: refused.stderr });
  }

  const sync = run(binary, ['product', '--workspace', workspace, 'sync', '--surface', 'cli', '--host', named, '--dry-run'], home);
  assert.ok(sync.status && !sync.error && !sync.signal, 'sync --surface cli --host must be refused');
  assert.match(sync.stderr, new RegExp(
    `the cli surface is reconciled on the machine that runs this command, so --host ${named} would be ignored`));
  assert.ok(!existsSync(receipt), 'a refused sync must leave no installation receipt');
  report.cases.push({ case: 'sync-cli-host', refusal: sync.stderr });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'install, update and sync of a cli surface given --host, refused in an isolated home before any build; not a service installation, a forwarded host installation or a GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
