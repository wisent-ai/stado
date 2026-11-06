import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { pinnedGit } from '../../native/home.mjs';
import { fileURLToPath } from 'node:url';

// Real journey of two automatic syncs that overlap, as the host's routine
// starts the cli sync while the desktop sync still builds. The Stado under
// test runs with HOME set to an isolated directory under .build, so its
// receipts and locks are never the person's; the workspace is the real one.
//
// 1. `product sync --surface desktop` starts and takes ~/.stado/products/sync.lock.
// 2. `product sync --surface cli` runs while it holds the lock: it must exit 0,
//    print `{"state": "busy"}` naming the lock and the first sync's pid as the
//    holder, and print no `Error:`; a busy lock is an answer, not a failure.
// Required: STADO_BIN, STADO_SYNC_TEST_SOURCE (Stado's checkout, whose parent
// directory is the workspace).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
const source = process.env.STADO_SYNC_TEST_SOURCE;
assert.ok(stado && source, 'STADO_BIN and STADO_SYNC_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-sync-busy-'));
const workspace = dirname(realpathSync(source));
const home = join(output, 'home');
mkdirSync(join(home, '.stado'), { recursive: true });
const env = { ...process.env, ...pinnedGit, HOME: home };
const report = { started_at: new Date().toISOString(), source, workspace, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const lock = join(home, '.stado', 'products', 'sync.lock');
function run(args) {
  const answer = spawnSync(resolve(stado), args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ arguments: args, exit_status: answer.status, signal: answer.signal,
    stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
let first;
let failure;
try {
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const version = run(['--version']);
  assert.equal(version.status, 0, version.stderr);
  report.stado = { sha256: digest(readFileSync(resolve(stado))), version: version.stdout.trim() };
  const lifecycle = surface => ['product', '--workspace', workspace, 'sync', '--surface', surface];
  first = spawn(resolve(stado), lifecycle('desktop'), { cwd: root, env });
  const kept = { arguments: lifecycle('desktop'), pid: first.pid, stderr: '' };
  first.stderr.on('data', chunk => { kept.stderr += chunk; });
  first.ended = new Promise(done => first.on('exit', (code, signal) => done(Object.assign(kept, { exit_status: code, signal }))));
  report.commands.push(kept);
  // The first sync holds the lock once its record names it.
  let holder = null;
  while (holder?.pid !== first.pid) {
    if (first.exitCode !== null || first.signalCode !== null) {
      throw new Error(`the first sync ended before it held ${lock}: ${kept.stderr.slice(-2000)}`);
    }
    try {
      holder = JSON.parse(readFileSync(lock, 'utf8'));
    } catch {
      holder = null;
    }
    if (holder?.pid !== first.pid) await new Promise(next => setTimeout(next, 200));
  }
  report.first_holder = holder;
  const second = run(lifecycle('cli'));
  assert.equal(second.status, 0, `a sync that finds the lock held must exit 0: ${second.stderr.slice(-2000)}`);
  assert.ok(!second.stderr.includes('Error:'), `a busy sync must print no error: ${second.stderr.slice(-2000)}`);
  const answer = JSON.parse(second.stdout.trim().split('\n').at(-1));
  assert.equal(answer.state, 'busy');
  assert.equal(answer.lock, lock, 'the busy answer names the lock it found held');
  assert.ok(answer.holder.includes(`pid ${first.pid}`), `the busy answer names the holder: ${answer.holder}`);
  report.busy = answer;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  if (first && first.exitCode === null && first.signalCode === null) {
    first.kill('SIGTERM');
    await first.ended;
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
