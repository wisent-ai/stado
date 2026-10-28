// Real journey of `stado build newest --queued` through the built Stado: the
// daily batch builds only the products whose handed-off work no build has
// taken yet. The store is a local store of this run (WC_STORAGE_BACKEND=local
// under .build/build-newest-queued, STADO_CONFIG the empty tests/config/empty.json, HOME of its
// own); two product checkouts each push to a bare origin of their own beside
// them, so a handoff names a commit on origin's main. `--plan` builds
// nothing.
//
// 1. Nothing handed off: `--queued` says no handed-off work is queued and
//    plans nothing.
// 2. One product's pushed commit is handed off with `stado release changes
//    submit`: `--queued --plan --json` plans that product and not the other,
//    while the plain plan names both.
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
const output = join(root, '.build', 'build-newest-queued', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const origins = join(output, 'origins');
const home = join(output, 'home');
const handed = `queued-handed-${randomUUID()}`;
const idle = `queued-idle-${randomUUID()}`;
const version = `${own.schema_version}.${own.schema_version}.${own.schema_version}`;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
mkdirSync(home, { recursive: true });
mkdirSync(origins, { recursive: true });
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
// A product checkout with one passing format gate, pushed to its own bare
// origin; its pushed commit.
function checkout(product) {
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
  ok('git', ['remote', 'add', 'origin', origin], directory);
  ok('git', ['push', '-q', 'origin', 'main'], directory);
  return ok('git', ['rev-parse', 'HEAD'], directory).trim();
}
const planned = args => JSON.parse(ok(stadoBinary, ['build', 'newest', '--root', workspace, '--plan', '--json', ...args], workspace))
  .products.map(entry => entry.product);

try {
  const commit = checkout(handed);
  checkout(idle);

  const nothing = ok(stadoBinary, ['build', 'newest', '--root', workspace, '--queued', '--plan'], workspace);
  assert.ok(nothing.includes('no handed-off work is queued; nothing to build'), nothing);

  ok(stadoBinary, ['release', 'changes', 'submit', '--source', join(workspace, handed), '--commit', commit,
    '--task', `task-${randomUUID()}`, '--session', randomUUID(), '--json'], workspace);
  const everything = planned([]);
  assert.ok(everything.includes(handed) && everything.includes(idle), everything.join(', '));
  assert.deepEqual(planned(['--queued']), [handed], 'the batch plans other than the products with queued work');
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
