import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// A Vast.ai offer carries the end date Stado chose: the real `stado market
// list --provider vast` on a host whose key Vast accepts lists the machine
// with `--contract-hours`, the marketplace's own view of the machine is read
// back and its offer end must lie within that window, and the offer is
// withdrawn again. The refusals need no key: a Vast.ai listing without
// --contract-hours, with --contract-hours 0, and a peer listing with it.
// STADO_BIN selects the Stado under test; STADO_VAST_TEST_PRICE_GPU and
// STADO_VAST_TEST_PRICE_DISK the prices the test listing is offered at (the
// operator's, nothing assumed). Without them the live half is reported blocked.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'market-offer-end-date-'));
const stado = process.env.STADO_BIN;
const priceGpu = process.env.STADO_VAST_TEST_PRICE_GPU;
const priceDisk = process.env.STADO_VAST_TEST_PRICE_DISK;
const hours = 2;
const report = { started_at: new Date().toISOString(), contract_hours: hours, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(exitedCleanly(answer), JSON.stringify({ stderr: answer.stderr, error: answer.error?.message,
    signal: answer.signal, status: answer.status }));
  return answer.stdout.trim();
}
function refused(answer, word) {
  assert.ok(!exitedCleanly(answer), `accepted: ${answer.stdout}`);
  assert.ok(`${answer.stderr}${answer.stdout}`.includes(word), `the refusal does not name ${word}: ${answer.stderr}`);
}
let listed = false;
let binary;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado) {
    report.result = 'blocked';
    throw new Error('requires STADO_BIN, the Stado under test');
  }
  report.stado = snapshotSubject(stado, output);
  binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);

  // Refusals, before anything is listed.
  refused(run(binary, ['market', 'list', '--provider', 'vast', '--price-gpu', '1', '--price-disk', '1']),
    '--contract-hours');
  refused(run(binary, ['market', 'list', '--provider', 'vast', '--price-gpu', '1', '--price-disk', '1',
    '--contract-hours', '0']), 'zero');
  refused(run(binary, ['market', 'list', '--provider', 'stado', '--peer', 'any', '--price-gpu', '1',
    '--contract-hours', '1']), '--contract-hours');

  if (!priceGpu || !priceDisk) {
    report.result = 'blocked';
    throw new Error('the live listing requires STADO_VAST_TEST_PRICE_GPU and STADO_VAST_TEST_PRICE_DISK');
  }
  const readiness = run(binary, ['market', 'readiness', '--provider', 'vast', '--json']);
  if (!exitedCleanly(readiness)) {
    report.result = 'blocked';
    throw new Error(`the host is not ready to list on Vast.ai: ${readiness.stdout}`);
  }
  const before = Date.now() / 1000;
  success(run(binary, ['market', 'list', '--provider', 'vast', '--price-gpu', priceGpu,
    '--price-disk', priceDisk, '--contract-hours', String(hours), '--json']));
  listed = true;
  const status = JSON.parse(success(run(binary, ['market', 'status', '--provider', 'vast', '--json'])));
  report.status = status;
  // Vast.ai's API reference names `end_date` for the create_asks request and
  // no field of the machine record; every field of the record that ends in
  // `end_date` is read, and one is required.
  const ends = Object.entries(status)
    .filter(([key, value]) => key.endsWith('end_date') && Number.isFinite(Number(value)))
    .map(([key, value]) => ({ key, value: Number(value) }));
  report.end_fields = ends;
  assert.ok(ends.length > 0,
    `the machine record states no *end_date field to read the offer's end from: ${Object.keys(status).join(', ')}`);
  for (const { key, value } of ends) {
    assert.ok(value > before && value <= before + hours * 3600 + 60,
      `${key} is ${value}, not within ${hours} h of ${before}`);
  }
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  if (listed) run(binary, ['market', 'unlist', '--provider', 'vast', '--json']);
  report.finished_at = new Date().toISOString();
  report.scope = 'the end date a Vast.ai offer carries, read back from the marketplace, and the refusals of --contract-hours';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
