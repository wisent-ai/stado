import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { watch, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// This journey replaces Stado only on a dedicated release host whose declared
// service-local reader's real update command is held by its remote channel.
// No simulated command, canned response or operator account is used.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const binary = process.env.STADO_BIN;
const host = process.env.STADO_TEST_DEDICATED_HOST;
const version = process.env.STADO_TEST_RELEASE_VERSION;
const revision = process.env.STADO_TEST_RELEASE_SOURCE_COMMIT;
const reader = process.env.STADO_TEST_CONVERGE_READER;
assert.ok(binary && host && version && revision && reader,
  'STADO_BIN, STADO_TEST_DEDICATED_HOST, STADO_TEST_RELEASE_VERSION, STADO_TEST_RELEASE_SOURCE_COMMIT and STADO_TEST_CONVERGE_READER declare the isolated real journey');
const program = realpathSync(binary);
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-converge-wait-'));
const report = { host, reader, version, revision, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function run(args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ executable: program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function succeeded(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function waits() { return JSON.parse(succeeded(run(['waits', '--json']))); }
function status() {
  return JSON.parse(succeeded(run(['product', 'status', 'stado', '--surface', 'cli', '--json'])));
}

let installing;
let failure;
try {
  const source = spawnSync('git', ['-C', root, 'rev-parse', 'HEAD'], { encoding: 'utf8' });
  report.commands.push({ executable: 'git', arguments: ['-C', root, 'rev-parse', 'HEAD'],
    exit_status: source.status, signal: source.signal, stderr: source.stderr, stdout: source.stdout });
  report.source_revision = succeeded(source);
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.binary_sha256 = digest(readFileSync(program));
  assert.equal(succeeded(run(['registry', 'self', '--local', '--name-only'])), host,
    'the dedicated host must be this machine');
  report.before = status();
  const args = ['product', 'install', 'stado', '--surface', 'cli',
    '--release-version', version, '--source-commit', revision];
  installing = spawn(program, args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
  const started = { executable: program, arguments: args, stdout: '', stderr: '' };
  report.commands.push(started);
  installing.stdout.on('data', chunk => { started.stdout += chunk; });
  installing.stderr.on('data', chunk => { started.stderr += chunk; });
  const exit = once(installing, 'exit').then(([code, signal]) => {
    started.exit_status = code;
    started.signal = signal;
    return { code, signal };
  });
  await Promise.race([
    new Promise((resolve, reject) => {
      installing.stderr.on('data', () => {
        if (/czekam:.*stado\/cli: product finish.*; gdzie: host /.test(started.stderr)) resolve();
      });
      installing.on('error', reject);
    }),
    exit.then(result => { throw new Error(`install ended before its converge wait: ${JSON.stringify(result)} ${started.stderr}`); }),
  ]);
  const initial = waits();
  const hasReader = listing => listing.open.some(entry => entry.line.includes(`reader ${reader}`)
    && entry.line.includes(`host ${host}`) && entry.line.includes('rodzaj: host'));
  const changes = watch(initial.register);
  let pending = once(changes, 'change');
  let standing = waits();
  try {
    while (!hasReader(standing)) {
      await Promise.race([
        pending,
        exit.then(result => { throw new Error(`install exited before remote reader wait: ${JSON.stringify(result)} ${started.stderr}`); }),
      ]);
      pending = once(changes, 'change');
      standing = waits();
    }
  } finally { changes.close(); }
  report.open_waits = standing.open;
  const operation = status().operation;
  assert.equal(operation.phase, 'after_install');
  assert.equal(operation.running, true);
  assert.ok(operation.steps.some(step => step.join(' ').includes('release local converge')));
  const cancelled = JSON.parse(succeeded(run(['product', 'cancel', 'stado', '--surface', 'cli', '--json'])));
  assert.equal(cancelled.cancelled.operation.pid, operation.pid);
  assert.ok(cancelled.cancelled.signalled.some(signal => signal.pid === operation.pid && signal.told_to_stop));
  report.cancellation = cancelled;
  const ended = await exit;
  assert.ok(ended.code || ended.signal, 'a cancelled real install must not report success');
  const after = status();
  assert.equal(after.operation.running, false, 'the operation no longer owns the recorded process');
  report.after = after;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  if (installing && installing.exitCode === null && installing.signalCode === null) {
    const cleanup = run(['product', 'cancel', 'stado', '--surface', 'cli', '--json']);
    report.cleanup = { exit_status: cleanup.status, stderr: cleanup.stderr, stdout: cleanup.stdout };
    if (cleanup.status || cleanup.error) installing.kill('SIGTERM');
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
