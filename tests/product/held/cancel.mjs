import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of an installation operation that holds a product surface's
// lock, as another installation, `stado product status` and `stado product
// cancel` see it. The Stado under test runs with HOME set to an isolated
// directory under .build, so the receipts and locks it writes are never the
// person's; the workspace is the real one, read for the product's checkout.
//
// 1. A source install of STADO_PRODUCT_TEST_PRODUCT (cli surface) at
//    STADO_PRODUCT_TEST_COMMIT starts; `status --json` names it under
//    `operation`: its pid, command, since when, phase and step.
// 2. A second install of the same surface with --wait names that holder on
//    stderr while it waits: pid, `running`, and the step.
// 3. `cancel --json` ends the first through its owner: it names the holder
//    and each process it told to stop; the first install's process ends; the
//    receipt records `cancelled` with the holder's step.
// 4. A second `cancel` is refused: nothing holds the surface.
// Required: STADO_BIN, STADO_PRODUCT_TEST_PRODUCT, STADO_PRODUCT_TEST_SOURCE.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && product && source,
  'STADO_BIN, STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-held-cancel-'));
const workspace = dirname(realpathSync(source));
const home = join(output, 'home');
mkdirSync(join(home, '.stado'), { recursive: true });
const env = { ...process.env, HOME: home };
const report = { started_at: new Date().toISOString(), product, source, workspace, commands: [], result: 'failed' };
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
function started(program, args) {
  const child = spawn(program, args, { cwd: root, env });
  const kept = { program, arguments: args, pid: child.pid, stderr: '' };
  child.stderr.on('data', chunk => { kept.stderr += chunk; });
  child.ended = new Promise(done => child.on('exit', (code, signal) => {
    Object.assign(kept, { exit_status: code, signal });
    done(kept);
  }));
  report.commands.push(kept);
  return { child, kept };
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const commit = process.env.STADO_PRODUCT_TEST_COMMIT || success(run('git', ['-C', source, 'rev-parse', 'origin/main']));
  const lifecycle = verb => ['product', '--workspace', workspace, verb, product, '--surface', 'cli'];
  const install = [...lifecycle('install'), '--source-commit', commit];

  const first = started(binary, install);
  let operation = null;
  while (!operation) {
    operation = JSON.parse(success(run(binary, [...lifecycle('status'), '--json']))).operation;
  }
  report.operation = operation;
  assert.equal(operation.pid, first.child.pid, 'status names another holder than the running install');
  assert.equal(operation.running, true);
  assert.ok(operation.acquired_at && operation.command.includes('install'), JSON.stringify(operation));

  const second = started(binary, [...install, '--wait']);
  while (!second.kept.stderr.includes(`held by pid ${first.child.pid} (running)`)) {
    await new Promise(next => second.child.stderr.once('data', next));
  }
  assert.match(second.kept.stderr, /in step /, 'the waiting install does not name the holder\'s step');

  const cancelled = JSON.parse(success(run(binary, [...lifecycle('cancel'), '--json'])));
  report.cancelled = cancelled;
  assert.equal(cancelled.cancelled.holder.pid, first.child.pid);
  assert.ok(cancelled.cancelled.signalled.some(told => told.pid === first.child.pid && told.told_to_stop));
  const ended = await first.child.ended;
  assert.equal(ended.signal, 'SIGTERM', 'the cancelled install did not end on the stop it was told');
  second.child.kill('SIGTERM');
  await second.child.ended;

  const receipt = JSON.parse(success(run(binary, [...lifecycle('status'), '--json'])));
  assert.equal(receipt.cancelled.holder.pid, first.child.pid, 'the receipt does not record the cancelled operation');
  assert.ok('step' in receipt.cancelled.holder);

  const again = run(binary, [...lifecycle('cancel'), '--json']);
  assert.ok(again.status && !again.error, 'a cancel with nothing to cancel was accepted');
  assert.match(again.stderr, new RegExp(`no installation operation holds ${product}/cli`));
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
