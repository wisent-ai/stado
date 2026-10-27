import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of a product surface lock whose record names a holder that
// has exited while another process holds the lock, as `stado product locks
// --json` reports it. The Stado under test runs with HOME set to an
// isolated directory under .build, so the lock it reads is never the
// person's.
//
// 1. The record names the pid of a process this journey started and saw
//    exit; a second process holds the lock file's flock and keeps it, and a
//    third has the file open without the flock, as a waiter does.
// 2. The lock's row is held, not running, and its holder sentence says the
//    record is stale and names both processes as having the lock file open,
//    the flock holder among them, by pid: the open-file table does not tell
//    the flock holder from a waiter.
// 3. Without them the row is free and names no holder.
// Required: STADO_BIN (the Stado under test); python3 holds the flock.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: it names the Stado whose lock report this journey qualifies');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-held-stale-'));
const home = join(output, 'home');
const lock = join(home, '.stado/products/stale-fixture/cli.lock');
mkdirSync(dirname(lock), { recursive: true });
const env = { ...process.env, HOME: home };
const report = { started_at: new Date().toISOString(), home, lock, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const row = () => {
  const answer = JSON.parse(success(run(stado, ['product', 'locks', '--json'])));
  const found = answer.locks.find(entry => entry.path === lock);
  assert.ok(found, `product locks names no row for ${lock}: ${JSON.stringify(answer)}`);
  return found;
};
let holder;
let waiter;
let failure;
try {
  report.source_revision = success(spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { sha256: digest(readFileSync(success(run('which', [stado])))), version: success(run(stado, ['--version'])) };

  const exited = spawnSync('true');
  assert.ok(exited.pid && !exited.status, 'the process whose pid the record names did not run');
  report.dead_pid = exited.pid;
  writeFileSync(lock, JSON.stringify({ product: 'stale-fixture', pid: exited.pid, started_at: 'its own birth time',
    command: 'stado product install stale-fixture --surface cli', acquired_at: new Date().toISOString(), phase: 'preparing' }));

  // The process that holds the flock until its standard input closes.
  holder = spawn('python3', ['-c', [
    'import fcntl, sys',
    `f = open(${JSON.stringify(lock)}, "a")`,
    'fcntl.flock(f, fcntl.LOCK_EX)',
    'print("locked", flush=True)',
    'sys.stdin.read()',
  ].join('\n')], { stdio: ['pipe', 'pipe', 'inherit'] });
  await new Promise((settled, refused) => {
    holder.stdout.on('data', chunk => String(chunk).includes('locked') && settled());
    holder.on('exit', code => refused(new Error(`the flock holder exited before locking: ${code}`)));
  });
  report.holder_pid = holder.pid;

  // A process with the lock file open and no flock, as a waiting install.
  waiter = spawn('python3', ['-c', [
    'import sys',
    `f = open(${JSON.stringify(lock)}, "a")`,
    'print("open", flush=True)',
    'sys.stdin.read()',
  ].join('\n')], { stdio: ['pipe', 'pipe', 'inherit'] });
  await new Promise((settled, refused) => {
    waiter.stdout.on('data', chunk => String(chunk).includes('open') && settled());
    waiter.on('exit', code => refused(new Error(`the waiter exited before opening the lock file: ${code}`)));
  });
  report.waiter_pid = waiter.pid;

  const held = row();
  report.held = held;
  assert.equal(held.held, true, 'the lock its holder keeps was reported free');
  assert.equal(held.running, false, 'the exited holder was reported running');
  assert.match(held.holder, /no longer running, so its record is stale/);
  assert.match(held.holder, /these processes have the lock file open, the flock holder among them/);
  assert.match(held.holder, new RegExp(`pid ${holder.pid} `), 'the flock holder was not named');
  assert.match(held.holder, new RegExp(`pid ${waiter.pid} `), 'the process with the file open was not named');

  waiter.stdin.end();
  await new Promise(settled => waiter.on('exit', settled));

  holder.stdin.end();
  await new Promise(settled => holder.on('exit', settled));
  const free = row();
  report.free = free;
  assert.equal(free.held, false, 'the lock was reported held after its holder exited');
  assert.equal(free.holder, null, 'a free lock named a holder');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  for (const child of [holder, waiter]) if (child && child.exitCode === null) child.kill();
  report.finished_at = new Date().toISOString();
  report.scope = 'One isolated home with a surface lock whose record names an exited pid while another process holds its flock; product locks --json only';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
