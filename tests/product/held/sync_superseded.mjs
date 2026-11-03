import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of an explicit installation started while the host's automatic
// `product sync` is building the same surface. The Stado under test runs with
// HOME set to an isolated directory under .build, so its receipts and locks
// are never the person's; the workspace is the real one.
//
// 1. `product sync --surface cli` starts and takes STADO_SYNC_TEST_PRODUCT's
//    cli lock while it prepares (builds origin/main).
// 2. `product install <product> --surface cli --source-commit <commit> --wait`
//    starts; it must say `superseding the automatic product sync` and take the
//    lock, the sync must end on the stop it was told, and the install must not
//    wait behind the sync's whole build.
// Required: STADO_BIN, STADO_SYNC_TEST_PRODUCT, STADO_SYNC_TEST_SOURCE (its
// checkout), STADO_SYNC_TEST_COMMIT.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
const product = process.env.STADO_SYNC_TEST_PRODUCT;
const source = process.env.STADO_SYNC_TEST_SOURCE;
const commit = process.env.STADO_SYNC_TEST_COMMIT;
assert.ok(stado && product && source && commit,
  'STADO_BIN, STADO_SYNC_TEST_PRODUCT, STADO_SYNC_TEST_SOURCE and STADO_SYNC_TEST_COMMIT are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-sync-superseded-'));
const workspace = dirname(realpathSync(source));
const home = join(output, 'home');
mkdirSync(join(home, '.stado'), { recursive: true });
const env = { ...process.env, HOME: home };
const report = { started_at: new Date().toISOString(), product, source, commit, workspace, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(args) {
  const answer = spawnSync(resolve(stado), args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ arguments: args, exit_status: answer.status, signal: answer.signal,
    stdout: answer.stdout, stderr: answer.stderr });
  assert.ok(!answer.status && !answer.signal, answer.stderr);
  return answer.stdout.trim();
}
const children = [];
function started(args) {
  const child = spawn(resolve(stado), args, { cwd: root, env });
  const kept = { arguments: args, pid: child.pid, stderr: '' };
  child.stderr.on('data', chunk => { kept.stderr += chunk; });
  child.ended = new Promise(done => child.on('exit', (code, signal) => {
    Object.assign(kept, { exit_status: code, signal });
    done(kept);
  }));
  report.commands.push(kept);
  children.push(child);
  return { child, kept };
}
async function said(one, text) {
  while (!one.kept.stderr.includes(text)) {
    if (one.child.exitCode !== null || one.child.signalCode !== null) {
      throw new Error(`pid ${one.child.pid} ended before saying ${JSON.stringify(text)}: ${one.kept.stderr.slice(-2000)}`);
    }
    await new Promise(next => one.child.stderr.once('data', next));
  }
}
let failure;
try {
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { sha256: digest(readFileSync(resolve(stado))), version: run(['--version']) };
  const lifecycle = verb => ['product', '--workspace', workspace, verb];
  const sync = started([...lifecycle('sync'), '--surface', 'cli']);
  let holder = null;
  while (holder?.pid !== sync.child.pid) {
    holder = JSON.parse(run([...lifecycle('status'), product, '--surface', 'cli', '--json'])).operation;
    if (holder?.pid !== sync.child.pid) await new Promise(next => setTimeout(next, 1000));
  }
  report.sync_holder = holder;
  const install = started([...lifecycle('install'), product, '--surface', 'cli', '--source-commit', commit, '--wait']);
  await said(install, 'superseding the automatic product sync');
  const ended = await sync.child.ended;
  assert.equal(ended.signal, 'SIGTERM', `the sync did not end on the stop it was told: ${ended.stderr.slice(-2000)}`);
  let now = null;
  while (now?.pid !== install.child.pid) {
    now = JSON.parse(run([...lifecycle('status'), product, '--surface', 'cli', '--json'])).operation;
    if (now?.pid !== install.child.pid) await new Promise(next => setTimeout(next, 1000));
  }
  report.install_holder = now;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  for (const child of children) {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
