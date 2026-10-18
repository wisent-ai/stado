// Real journey of the delivery queue's serve role through the built Stado:
// `stado serve --release-loop-root WORKSPACE` queues every product checkout
// under the workspace at start, and again when a checkout's branch moves,
// with nothing polled. A workspace of its own under
// .build/release-delivery-queue-serve holds one product checkout, a real Git
// repository with a release manifest of the schema Stado's own manifest
// declares; Oko's defect register (OKO_DEFECT_REGISTER, empty) and the
// queue's register (STADO_RELEASE_LOOP_REGISTER) are files of this run.
//
// 1. At start the role keeps the checkout's commit as nothing_carried.
// 2. A second commit moves the branch; the role keeps that commit too,
//    without being asked, and the first entry stays as it was.
// 3. The role builds and commits nothing: the checkout's HEAD is the commit
//    the journey made, and the register holds no other state.
// The serve process is ended by the journey when the second entry lands.
//
// Needs STADO_BIN and OKO_BIN.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, realpathSync, watch, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const stadoBinary = realpathSync(required('STADO_BIN'));
const okoBinary = realpathSync(required('OKO_BIN'));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const own = JSON.parse(readFileSync(join(root, '.wisent-release.json'), 'utf8'));
const output = join(root, '.build', 'release-delivery-queue-serve', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const product = `queue-fixture-${randomUUID()}`;
const checkout = join(workspace, product);
mkdirSync(checkout, { recursive: true });
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
  commands: [], serve: { stderr: '' }, verdict: 'failed',
};
function git(args) {
  const result = spawnSync('git', args, { cwd: checkout, env: environment, encoding: 'utf8' });
  report.commands.push({ program: 'git', args, status: result.status, stderr: result.stderr });
  assert.ok(exitedCleanly(result), `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}
function entries() {
  try {
    return JSON.parse(readFileSync(environment.STADO_RELEASE_LOOP_REGISTER, 'utf8')).entries;
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
}
// Resolves once the register holds an entry for `commit`, woken by the
// kernel's notification on the register's directory; rejects when serve
// ends first, with what it said.
function entryFor(commit, serve) {
  return new Promise((resolveEntry, rejectEntry) => {
    const look = () => entries().find(entry => entry.product === product && entry.commit === commit);
    const watcher = watch(output, () => {
      const found = look();
      if (found) { watcher.close(); resolveEntry(found); }
    });
    serve.once('exit', code => {
      watcher.close();
      rejectEntry(new Error(`stado serve ended (${code}) before queueing ${commit}: ${report.serve.stderr}`));
    });
    const found = look();
    if (found) { watcher.close(); resolveEntry(found); }
  });
}

let serve;
try {
  git(['init', '-q', '-b', 'main']);
  writeFileSync(join(checkout, 'VERSION'), `${own.schema_version}.${own.schema_version}.${own.schema_version}\n`);
  writeFileSync(join(checkout, '.wisent-release.json'), JSON.stringify({
    schema_version: own.schema_version, product, releases: true,
    version_source: { kind: 'text', path: 'VERSION' },
    platforms: {}, promotion: { channels: ['candidate'], reconcile: false }, deliveries: [],
  }, null, '  '));
  git(['add', '.']);
  git(['commit', '-q', '-m', 'fixture']);
  const first = git(['rev-parse', 'HEAD']);

  serve = spawn(stadoBinary, ['serve', '--release-loop-root', workspace], { cwd: workspace, env: environment });
  serve.stderr.on('data', chunk => { report.serve.stderr += chunk; });
  const atStart = await entryFor(first, serve);
  assert.equal(atStart.state, 'nothing_carried');

  writeFileSync(join(checkout, 'NOTE'), 'second commit\n');
  git(['add', 'NOTE']);
  git(['commit', '-q', '-m', 'second']);
  const second = git(['rev-parse', 'HEAD']);
  const afterMove = await entryFor(second, serve);
  assert.equal(afterMove.state, 'nothing_carried');

  assert.equal(git(['rev-parse', 'HEAD']), second, 'the role committed');
  assert.deepEqual(entries().find(entry => entry.commit === first), atStart, 'the first entry changed');
  assert.deepEqual([...new Set(entries().map(entry => entry.state))], ['nothing_carried']);
  report.verdict = 'passed';
} finally {
  if (serve && serve.exitCode === null) serve.kill('SIGTERM');
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
