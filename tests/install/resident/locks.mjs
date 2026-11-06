// A dedicated-host journey through the real product CLI and an isolated HOME.
// It tests a released lock left by a dependency install, a held lock whose
// guarded product disagrees with its record, and `stado product cancel`
// ending a real lock holder.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { isolatedPath, pinnedGit } from '../../native/home.mjs';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const program = process.env.STADO_BIN;
const revision = process.env.STADO_TEST_SOURCE_COMMIT;
const host = process.env.STADO_TEST_DEDICATED_HOST;
assert.ok(program && revision && host && process.env.HOME,
  'STADO_BIN, STADO_TEST_SOURCE_COMMIT, STADO_TEST_DEDICATED_HOST and HOME are required');
const output = join(root, '.build', 'install-locks', createHash('sha256')
  .update(`${revision}:${host}`).digest('hex'));
const home = join(output, 'home');
mkdirSync(home, { recursive: true });
const ciRecord = process.env.STADO_CI_MAIN_RECORD
  ? process.env.STADO_CI_MAIN_RECORD : join(process.env.HOME, '.stado', 'ci-main.json');
// The isolated home's ~/.local/bin leads PATH, or each finish refuses to
// change which `stado` the person's own ~/.local/bin runs.
const environment = { ...process.env, ...pinnedGit, HOME: home, WISENT_WORKSPACE: dirname(root),
  STADO_CI_MAIN_RECORD: ciRecord, PATH: isolatedPath(home) };
const report = { result: 'failed', source_revision: revision, host, ci_record: ciRecord, commands: [] };
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
function run(arguments_, env = environment) {
  const result = spawnSync(program, arguments_, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ executable: program, arguments: arguments_, exit_status: result.status,
    signal: result.signal, stdout: result.stdout, stderr: result.stderr,
    error: result.error?.message });
  return result;
}
function succeeds(result) {
  assert.ok(!result.error && !result.status && !result.signal,
    `command failed: ${result.error?.message} ${result.stderr} ${result.stdout}`);
  return result.stdout.trim();
}
function record(path, product, pid, started, phase = 'preparing') {
  writeFileSync(path, `${JSON.stringify({ product, pid, started_at: started,
    command: `${program} product install ${product} --surface cli`,
    acquired_at: new Date().toISOString(), phase, step: 'source build' })}\n`);
}
function birthOf(pid) {
  const birth = spawnSync('/bin/ps', ['-p', String(pid), '-o', 'lstart='], { encoding: 'utf8' });
  report.commands.push({ executable: '/bin/ps', arguments: ['-p', String(pid), '-o', 'lstart='],
    exit_status: birth.status, stdout: birth.stdout, stderr: birth.stderr });
  assert.ok(!birth.status && birth.stdout.trim(), `cannot read the birth of pid ${pid}: ${birth.stderr}`);
  return birth.stdout.trim();
}
function refuses(result, pattern, what) {
  assert.ok(result.status || result.signal || result.error, `${what} must refuse`);
  assert.match(result.stderr, pattern);
  return { exit_status: result.status, stderr: result.stderr };
}
let failure;
try {
  report.binary_sha256 = hash(program);
  report.test_sha256 = hash(fileURLToPath(import.meta.url));
  assert.equal(succeeds(run(['registry', 'self', '--local', '--name-only'], process.env)), host,
    'the dedicated host must be this machine');
  const lock = join(home, '.stado', 'products', 'stado', 'cli.lock');
  mkdirSync(dirname(lock), { recursive: true });
  const former = spawn('/bin/cat', [], { stdio: ['pipe', 'ignore', 'pipe'] });
  await once(former, 'spawn');
  const birth = spawnSync('/bin/ps', ['-p', String(former.pid), '-o', 'lstart='],
    { encoding: 'utf8' });
  report.commands.push({ executable: '/bin/ps', arguments: ['-p', String(former.pid), '-o', 'lstart='],
    exit_status: birth.status, stdout: birth.stdout, stderr: birth.stderr });
  assert.ok(!birth.status && birth.stdout.trim(), `cannot read holder birth: ${birth.stderr}`);
  const departed = once(former, 'exit');
  former.kill();
  await departed;
  record(lock, 'oko', former.pid, birth.stdout.trim());
  const before = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  const stale = before.locks.find(row => row.path === lock);
  assert.equal(stale.held, false);
  assert.equal(stale.running, false);
  assert.equal(stale.recorded_product, 'oko');
  report.before = before;
  const installed = run(['product', 'install', 'stado', '--surface', 'cli',
    '--source-commit', revision, '--with-prerequisites', '--json']);
  succeeds(installed);
  assert.match(installed.stderr, /reclaiming released lock .*recorded product Some\("oko"\).*command/);
  const after = JSON.parse(succeeds(run(['product', 'status', 'stado', '--surface', 'cli', '--json'])));
  assert.equal(after.source_revision, revision);
  report.after = after;
  const reclaimed = JSON.parse(readFileSync(lock, 'utf8'));
  assert.equal(reclaimed.product, 'stado');
  assert.ok(reclaimed.started_at);
  const fixture = join(output, 'lock-holder');
  // rustc by its rustup proxy's path: the isolated PATH leaves out
  // ~/.cargo/bin, which also links the person's stado.
  const rustc = join(homedir(), '.cargo/bin/rustc');
  const compiled = spawnSync(rustc, ['--edition', '2021',
    join(dirname(fileURLToPath(import.meta.url)), 'lock_holder.rs'), '-o', fixture],
    { cwd: root, env: { ...environment, PATH: process.env.PATH }, encoding: 'utf8' });
  report.commands.push({ executable: rustc, arguments: compiled.spawnargs,
    exit_status: compiled.status, stdout: compiled.stdout, stderr: compiled.stderr });
  succeeds(compiled);
  const live = spawn(fixture, [lock], { stdio: ['pipe', 'pipe', 'pipe'] });
  const ready = await Promise.race([
    once(live.stdout, 'data').then(([data]) => data.toString().trim()),
    once(live, 'exit').then(([code]) => { throw new Error(`lock holder exited before ready: ${code}`); }),
  ]);
  assert.equal(ready, 'held');
  record(lock, 'oko', former.pid, birth.stdout.trim());
  const held = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  assert.equal(held.locks.find(row => row.path === lock).held, undefined);
  assert.match(held.locks.find(row => row.path === lock).error,
    /guards product stado but records product oko; held lock is corrupt/);
  const refused = run(['product', 'install', 'stado', '--surface', 'cli',
    '--source-commit', revision, '--json']);
  assert.ok(refused.status || refused.signal || refused.error, 'held mislabelled lock must refuse');
  assert.match(refused.stderr, /guards product stado but records product oko; held lock is corrupt/);
  report.held = held;
  report.refusal = { exit_status: refused.status, stderr: refused.stderr };
  const released = once(live, 'exit');
  live.stdin.end('\n');
  const [holderStatus] = await released;
  assert.equal(holderStatus, installed.status, 'the real file lock holder released normally');
  report.commands.push({ executable: fixture, arguments: [lock], exit_status: holderStatus });
  const free = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  assert.equal(free.locks.find(row => row.path === lock).held, false,
    'the CLI must observe the released OS file lock');
  assert.equal(free.locks.find(row => row.path === lock).recorded_product, 'oko');
  report.free = free;
  const reclaimedAgain = run(['product', 'install', 'stado', '--surface', 'cli',
    '--source-commit', revision, '--json']);
  succeeds(reclaimedAgain);
  assert.match(reclaimedAgain.stderr, /reclaiming released lock .*recorded product Some\("oko"\)/);
  const finalRecord = JSON.parse(readFileSync(lock, 'utf8'));
  assert.equal(finalRecord.product, 'stado');
  report.finalRecord = finalRecord;
  const cancel = ['product', 'cancel', 'stado', '--surface', 'cli', '--json'];
  report.cancelIdle = refuses(run(cancel), /nothing to cancel on stado\/cli/, 'cancel with nothing running');
  report.cancelHost = refuses(run([...cancel, '--host', host]), /reads no --host/, 'cancel with --host');
  const holder = spawn(fixture, [lock], { stdio: ['pipe', 'pipe', 'pipe'] });
  const holding = await Promise.race([
    once(holder.stdout, 'data').then(([data]) => data.toString().trim()),
    once(holder, 'exit').then(([code]) => { throw new Error(`lock holder exited before ready: ${code}`); }),
  ]);
  assert.equal(holding, 'held');
  const holderBirth = birthOf(holder.pid);
  record(lock, 'stado', holder.pid, holderBirth, 'placing');
  report.cancelPlacing = refuses(run(cancel), /is being placed/, 'cancel of a holder placing files');
  assert.equal(holder.exitCode, null, 'a holder placing files must not be stopped');
  record(lock, 'stado', holder.pid, holderBirth, 'preparing');
  const ended = once(holder, 'exit');
  const cancelled = JSON.parse(succeeds(run(cancel)));
  const [code, signal] = await ended;
  report.commands.push({ executable: fixture, arguments: [lock], exit_status: code, signal });
  assert.equal(signal, 'SIGTERM', 'cancel must stop the holder with SIGTERM');
  assert.equal(cancelled.cancelled, 'lock holder');
  assert.equal(cancelled.operation.pid, holder.pid);
  assert.ok(cancelled.operation.told.some(told => told.pid === holder.pid && told.told_to_stop));
  const afterCancel = JSON.parse(succeeds(run(['product', 'status', 'stado', '--surface', 'cli', '--json'])));
  assert.equal(afterCancel.operation_cancelled.pid, holder.pid, 'the receipt records the cancelled holder');
  assert.ok(afterCancel.operation_cancelled.cancelled_by.pid, 'the receipt names the process that asked');
  assert.equal(afterCancel.operation, null, 'nothing holds the surface after cancel');
  const freed = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  assert.equal(freed.locks.find(row => row.path === lock).held, false, 'cancel releases the lock');
  report.cancelled = cancelled;
  report.afterCancel = afterCancel;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
