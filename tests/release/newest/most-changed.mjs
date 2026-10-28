// Real journey of `stado build newest --most-changed` through the built Stado:
// the fleet's one build of the day goes to the product that carries the most
// commits past its last passed build, never to the one asked for first. The
// store is a local store of this run (WC_STORAGE_BACKEND=local under
// .build/build-newest-most-changed, STADO_CONFIG the empty tests/config/empty.json, HOME of its
// own), so no build of either product has passed and each one's change is its
// whole pushed history. Two product checkouts push to bare origins of their
// own beside them. `--plan` builds nothing.
//
// 1. An empty workspace: `--most-changed` says no product carries a commit
//    past its last passed build and plans nothing.
// 2. Both products pushed, the larger one with more commits: the plain plan
//    names both, `--most-changed --plan --json` names only the larger one,
//    and the text plan says how many commits it carries (git's own count).
// 3. The smaller one is named first on the command line: the choice does not
//    change, because the order products are asked for is not the measure.
//
// Needs STADO_BIN.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../../native/process.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const stadoBinary = realpathSync(required('STADO_BIN'));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const own = JSON.parse(readFileSync(join(root, '.wisent-release.json'), 'utf8'));
const output = join(root, '.build', 'build-newest-most-changed', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const empty = join(output, 'empty');
const origins = join(output, 'origins');
const home = join(output, 'home');
const smaller = `changed-smaller-${randomUUID()}`;
const larger = `changed-larger-${randomUUID()}`;
const version = `${own.schema_version}.${own.schema_version}.${own.schema_version}`;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
for (const directory of [home, origins, empty]) mkdirSync(directory, { recursive: true });
const environment = {
  ...process.env,
  HOME: home,
  STADO_CONFIG: join(root, 'tests', 'config', 'empty.json'),
  WC_STORAGE_BACKEND: 'local',
  WC_LOCAL_STORAGE_PATH: join(output, 'store'),
};
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  commands: [], verdict: 'failed',
};
function run(program, args, cwd) {
  const result = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}
function ok(program, args, cwd) {
  const result = run(program, args, cwd);
  assert.ok(exitedCleanly(result), `${program} ${args.join(' ')} was refused: ${result.stderr}`);
  return result.stdout;
}
// A product checkout with one passing format gate and one further commit per
// entry of `changes`, pushed to its own bare origin; git's count of the
// commits it holds.
function checkout(product, changes) {
  const origin = join(origins, `${product}.git`);
  ok('git', ['init', '-q', '--bare', '-b', 'main', origin], output);
  const directory = join(workspace, product);
  mkdirSync(directory, { recursive: true });
  ok('git', ['init', '-q', '-b', 'main'], directory);
  const passingGate = { name: 'passing-format', argv: ['sh', '-c', 'true'], kind: 'format' };
  const platforms = Object.fromEntries(Object.entries(own.platforms)
    .map(([name, recipe]) => [name, { ...recipe, quality: [passingGate] }]));
  writeFileSync(join(directory, 'VERSION'), `${version}\n`);
  writeFileSync(join(directory, '.wisent-release.json'), JSON.stringify({
    schema_version: own.schema_version, product, releases: true,
    version_source: { kind: 'text', path: 'VERSION' },
    platforms, promotion: { channels: ['candidate'], reconcile: false }, deliveries: [],
  }, null, '  '));
  ok('git', ['add', '.'], directory);
  ok('git', ['commit', '-q', '-m', 'fixture'], directory);
  for (const change of changes) {
    writeFileSync(join(directory, 'CHANGES'), `${change}\n`);
    ok('git', ['add', 'CHANGES'], directory);
    ok('git', ['commit', '-q', '-m', change], directory);
  }
  ok('git', ['remote', 'add', 'origin', origin], directory);
  ok('git', ['push', '-q', 'origin', 'main'], directory);
  return ok('git', ['rev-list', '--count', 'HEAD'], directory).trim();
}
const planned = (at, args) => JSON.parse(ok(stadoBinary, ['build', 'newest', '--root', at, '--plan', '--json', ...args], at))
  .products.map(entry => entry.product);

try {
  const nothing = ok(stadoBinary, ['build', 'newest', '--root', empty, '--most-changed', '--plan'], empty);
  assert.ok(nothing.includes('no product carries a commit past its last passed build; nothing to build'), nothing);

  checkout(smaller, []);
  const count = checkout(larger, [randomUUID(), randomUUID()]);
  const everything = planned(workspace, []);
  assert.ok(everything.includes(smaller) && everything.includes(larger), everything.join(', '));
  assert.deepEqual(planned(workspace, ['--most-changed']), [larger], 'the daily build went to the smaller change');

  const said = ok(stadoBinary, ['build', 'newest', '--root', workspace, '--most-changed', '--plan'], workspace);
  const carried = `${larger} carries the most change: ${count} commit(s) past its last passed build`;
  assert.ok(said.includes(carried), said);

  assert.deepEqual(planned(workspace, ['--most-changed', '--product', smaller, '--product', larger]), [larger],
    'naming the smaller product first took the day from the larger change');
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
