// A dedicated-host journey through the real product CLI and an isolated HOME.
// It leaves lock records as a terminated installer and a mislabelled installer
// would, then asks a real source install to acquire the surface and observes it.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const program = process.env.STADO_BIN;
const revision = process.env.STADO_TEST_SOURCE_COMMIT;
const host = process.env.STADO_TEST_DEDICATED_HOST;
assert.ok(program && revision && host,
  'STADO_BIN, STADO_TEST_SOURCE_COMMIT and STADO_TEST_DEDICATED_HOST are required');
const output = join(root, '.build', 'install-locks', createHash('sha256')
  .update(`${revision}:${host}`).digest('hex'));
const home = join(output, 'home');
mkdirSync(home, { recursive: true });
const environment = { ...process.env, HOME: home, WISENT_WORKSPACE: dirname(root) };
const report = { result: 'failed', source_revision: revision, host, commands: [] };
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
function record(path, product, pid, started) {
  writeFileSync(path, `${JSON.stringify({ product, pid, started_at: started,
    command: `${program} product install ${product} --surface cli`,
    acquired_at: new Date().toISOString(), phase: 'preparing', step: 'source build' })}\n`);
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
  record(lock, 'stado', former.pid, birth.stdout.trim());
  const before = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  assert.equal(before.locks.find(row => row.path === lock).running, false);
  report.before = before;
  const installed = run(['product', 'install', 'stado', '--surface', 'cli',
    '--source-commit', revision, '--json']);
  succeeds(installed);
  assert.match(installed.stderr, /reclaiming stale lock .*stado.*recorded pid/);
  const after = JSON.parse(succeeds(run(['product', 'status', 'stado', '--surface', 'cli', '--json'])));
  assert.equal(after.source_revision, revision);
  report.after = after;
  const reclaimed = JSON.parse(readFileSync(lock, 'utf8'));
  assert.equal(reclaimed.product, 'stado');
  assert.ok(reclaimed.started_at);
  const fixture = join(output, 'lock-holder');
  const compiled = spawnSync('rustc', ['--edition', '2021',
    join(dirname(fileURLToPath(import.meta.url)), 'lock_holder.rs'), '-o', fixture],
    { cwd: root, env: environment, encoding: 'utf8' });
  report.commands.push({ executable: 'rustc', arguments: compiled.spawnargs,
    exit_status: compiled.status, stdout: compiled.stdout, stderr: compiled.stderr });
  succeeds(compiled);
  const live = spawn(fixture, [lock], { stdio: ['pipe', 'pipe', 'pipe'] });
  const ready = await Promise.race([
    once(live.stdout, 'data').then(([data]) => data.toString().trim()),
    once(live, 'exit').then(([code]) => { throw new Error(`lock holder exited before ready: ${code}`); }),
  ]);
  assert.equal(ready, 'held');
  const held = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  assert.equal(held.locks.find(row => row.path === lock).held, true,
    'the CLI must observe another process holding the OS file lock');
  report.held = held;
  const released = once(live, 'exit');
  live.stdin.end('\n');
  const [holderStatus] = await released;
  assert.equal(holderStatus, installed.status, 'the real file lock holder released normally');
  report.commands.push({ executable: fixture, arguments: [lock], exit_status: holderStatus });
  const free = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  assert.equal(free.locks.find(row => row.path === lock).held, false,
    'the CLI must observe the released OS file lock');
  report.free = free;
  record(lock, 'tama', former.pid, birth.stdout.trim());
  const refused = run(['product', 'install', 'stado', '--surface', 'cli',
    '--source-commit', revision, '--json']);
  assert.ok(refused.status || refused.signal || refused.error, 'mislabelled lock must refuse');
  assert.match(refused.stderr, /guards product stado but records product tama; corrupt lock/);
  const corrupt = JSON.parse(succeeds(run(['product', 'locks', '--json'])));
  assert.match(corrupt.locks.find(row => row.path === lock).error,
    /guards product stado but records product tama/);
  report.refusal = { exit_status: refused.status, stderr: refused.stderr };
  report.corrupt = corrupt;
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
