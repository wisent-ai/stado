// Real journey of the delivery queue's order and of the repairs it cannot
// place, through the built Stado and the built Oko. A workspace of its own
// under .build/release-delivery-queue-order holds two product checkouts
// whose release manifests take Stado's own platform recipes with one format
// gate that passes (`sh -c true`); `--plan` builds nothing. Oko's defect
// register and the queue's register are files of this run.
//
// 1. `stado release newest --plan --first <second> --json` lists the second
//    product ahead of the first; `--first` naming no product of the plan is
//    refused, naming the products the plan holds.
// 2. Three open repairs no checkout carries are listed under `unplaced`,
//    each with why: one recorded at a commit that exists only on a side
//    branch (not in the history the checkout stands on), one recorded in a
//    repository the workspace holds no checkout of, and one at a commit the
//    checkout does not hold at all. A repair the checkout's commit carries is
//    not listed.
//
// Needs STADO_BIN, OKO_BIN and STADO_TEST_SUCCESS_EXIT.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const stadoBinary = realpathSync(required('STADO_BIN'));
const okoBinary = realpathSync(required('OKO_BIN'));
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const own = JSON.parse(readFileSync(join(root, '.wisent-release.json'), 'utf8'));
const output = join(root, '.build', 'release-delivery-queue-order', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const runId = randomUUID();
const alpha = `order-a-${runId}`;
const beta = `order-b-${runId}`;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = {
  ...process.env,
  PATH: `${dirname(okoBinary)}:${process.env.PATH}`,
  OKO_DEFECT_REGISTER: join(output, 'defects.json'),
  STADO_RELEASE_LOOP_REGISTER: join(output, 'release-loop.json'),
};
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  oko: okoBinary, oko_sha256: digest(readFileSync(okoBinary)),
  commands: [], verdict: 'failed',
};
function run(program, args, cwd) {
  const result = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}
function ok(program, args, cwd) {
  const result = run(program, args, cwd);
  assert.equal(result.status, successExit, `${program} ${args.join(' ')} was refused: ${result.stderr}`);
  return result.stdout;
}
function checkout(product) {
  const directory = join(workspace, product);
  mkdirSync(directory, { recursive: true });
  ok('git', ['init', '-q', '-b', 'main'], directory);
  const passingGate = { name: 'passing-format', argv: ['sh', '-c', 'true'], kind: 'format' };
  const platforms = Object.fromEntries(Object.entries(own.platforms)
    .map(([name, recipe]) => [name, { ...recipe, quality: [passingGate] }]));
  writeFileSync(join(directory, 'VERSION'), `${own.schema_version}.${own.schema_version}.${own.schema_version}\n`);
  writeFileSync(join(directory, '.wisent-release.json'), JSON.stringify({
    schema_version: own.schema_version, product, releases: true,
    version_source: { kind: 'text', path: 'VERSION' },
    platforms, promotion: { channels: ['candidate'], reconcile: false }, deliveries: [],
  }, null, '  '));
  ok('git', ['add', '.'], directory);
  ok('git', ['commit', '-q', '-m', 'fixture'], directory);
  return directory;
}
function repaired(revision) {
  const recorded = ok(okoBinary, [
    'defects', 'record', '--product', alpha, '--title', `delivery queue order fixture ${revision}`,
    '--surface', 'release queue', '--evidence', 'tests/release/delivery-queue-order.mjs',
    '--impact', 'none: test record', '--repair', 'the fixture commit', '--found-in', 'delivery-queue-order-test', '--json',
  ], workspace);
  const { id } = JSON.parse(recorded);
  ok(okoBinary, ['defects', 'repaired', id, '--revision', revision, '--detail', 'the fixture commit'], workspace);
  return id;
}
const plan = extra => JSON.parse(ok(stadoBinary, ['release', 'newest', '--root', workspace, '--plan', '--json', ...extra], workspace));

try {
  const first = checkout(alpha);
  checkout(beta);
  const stands = ok('git', ['rev-parse', 'HEAD'], first).trim();
  ok('git', ['switch', '-q', '-c', 'side'], first);
  writeFileSync(join(first, 'SIDE'), 'only on the side branch\n');
  ok('git', ['add', 'SIDE'], first);
  ok('git', ['commit', '-q', '-m', 'side'], first);
  const side = ok('git', ['rev-parse', 'HEAD'], first).trim();
  ok('git', ['switch', '-q', 'main'], first);
  const absent = digest(Buffer.from(runId)).slice(-stands.length);

  const carried = repaired(`${alpha}@${stands}`);
  const offHistory = repaired(`${alpha}@${side}`);
  const elsewhere = repaired(`no-checkout-${runId}@${stands}`);
  const missing = repaired(`${alpha}@${absent}`);

  const ordered = plan(['--first', beta]);
  assert.deepEqual(ordered.products.map(entry => entry.product), [beta, alpha]);
  const unplaced = Object.fromEntries(ordered.unplaced.map(entry => [entry.defect, entry]));
  assert.ok(!(carried in unplaced), `a carried repair is listed as unplaced: ${JSON.stringify(ordered.unplaced)}`);
  assert.ok(unplaced[offHistory].why.includes(`is not in the history of ${stands}`), unplaced[offHistory]?.why);
  assert.ok(unplaced[elsewhere].why.includes(`holds no checkout of no-checkout-${runId}`), unplaced[elsewhere]?.why);
  assert.ok(unplaced[missing].why.includes(`holds no commit ${absent}`), unplaced[missing]?.why);

  const refused = run(stadoBinary, ['release', 'newest', '--root', workspace, '--plan', '--first', 'no-such-product'], workspace);
  assert.notEqual(refused.status, successExit, 'an unknown --first was accepted');
  assert.ok(refused.stderr.includes('--first no-such-product names no product of this plan'), refused.stderr);
  assert.ok(refused.stderr.includes(alpha) && refused.stderr.includes(beta), refused.stderr);
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
