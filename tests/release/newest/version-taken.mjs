// Real journey of how `stado release newest --plan` reads a version that a
// run of another commit holds, through the built Stado. A published run once
// settled its version for every later commit, so a product moved on by many
// commits under one declared version read "already published" and none of
// them could be released. A run now settles only its own commit.
//
// The run records live in a local store of this run (WC_STORAGE_BACKEND=local
// under .build/release-version-taken, STADO_CONFIG naming no file, HOME of its
// own), written where the release pipeline writes them
// (`runs/release-pipeline/<run>/run.json`). One product checkout declares its
// version in VERSION; `--plan` builds nothing.
//
// 1. A published run of the checkout's commit: the commit stands `published`
//    by that run.
// 2. A later commit under the same declared version: it stands
//    `version_taken`, naming the run and the commit that hold the version and
//    the least next version; the text plan says it needs at least that
//    version before it can be released.
// 3. A failed run of another commit holds the version as well: a submission
//    attested that commit as the version's source.
// 4. A published run of the later commit itself: it stands `published` again.
// 5. A run record that names no source commit: the plan is refused naming the
//    record, instead of reading the version as taken by a commit nobody named.
//
// Needs STADO_BIN and OKO_BIN.
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
const okoBinary = realpathSync(required('OKO_BIN'));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const own = JSON.parse(readFileSync(join(root, '.wisent-release.json'), 'utf8'));
const output = join(root, '.build', 'release-version-taken', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const store = join(output, 'store');
const home = join(output, 'home');
const product = `version-taken-${randomUUID()}`;
const version = `${own.schema_version}.${own.schema_version}.${own.schema_version}`;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
mkdirSync(home, { recursive: true });
const environment = {
  ...process.env,
  HOME: home,
  PATH: `${dirname(okoBinary)}:${process.env.PATH}`,
  STADO_CONFIG: join(output, 'no-config.json'),
  WC_STORAGE_BACKEND: 'local',
  WC_LOCAL_STORAGE_PATH: store,
  OKO_DEFECT_REGISTER: join(output, 'defects.json'),
  STADO_RELEASE_LOOP_REGISTER: join(output, 'release-loop.json'),
};
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  oko: okoBinary, oko_sha256: digest(readFileSync(okoBinary)),
  commands: [], runs: [], verdict: 'failed',
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
function checkout() {
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
  return directory;
}
function commitOnward(directory, name) {
  writeFileSync(join(directory, name), `${name}\n`);
  ok('git', ['add', name], directory);
  ok('git', ['commit', '-q', '-m', name], directory);
  return ok('git', ['rev-parse', 'HEAD'], directory).trim();
}
// One run as the release pipeline records it.
function recorded(commit, state) {
  const runId = randomUUID();
  const path = join(store, 'runs', 'release-pipeline', runId, 'run.json');
  mkdirSync(dirname(path), { recursive: true });
  const body = { run_id: runId, product, version, state, source_commit: commit };
  writeFileSync(path, JSON.stringify(body, null, '  '));
  report.runs.push({ path, body });
  return runId;
}
const standing = () => {
  const answer = JSON.parse(ok(stadoBinary, ['release', 'newest', '--root', workspace, '--product', product, '--plan', '--json'], workspace));
  const entry = answer.products.find(planned => planned.product === product);
  assert.ok(entry, `the plan holds no entry for ${product}: ${JSON.stringify(answer)}`);
  return entry;
};

try {
  const directory = checkout();
  const first = ok('git', ['rev-parse', 'HEAD'], directory).trim();
  const firstRun = recorded(first, 'completed');

  const settled = standing();
  assert.equal(settled.standing, 'published', JSON.stringify(settled));
  assert.equal(settled.commit, first);
  assert.equal(settled.run, firstRun);

  const second = commitOnward(directory, 'LATER');
  const taken = standing();
  assert.equal(taken.standing, 'version_taken', `a later commit read as already published: ${JSON.stringify(taken)}`);
  assert.equal(taken.commit, second);
  assert.equal(taken.version, version);
  assert.equal(taken.run, firstRun);
  assert.equal(taken.taken_by, first);
  assert.equal(typeof taken.next, 'string', `no next version is named: ${JSON.stringify(taken)}`);
  assert.notEqual(taken.next, version, `the next version is the spent one: ${JSON.stringify(taken)}`);
  const text = ok(stadoBinary, ['release', 'newest', '--root', workspace, '--product', product, '--plan'], workspace);
  assert.ok(text.includes(`it needs at least ${taken.next}`), text);

  const third = commitOnward(directory, 'FAILED');
  const failedRun = recorded(third, 'failed');
  const fourth = commitOnward(directory, 'AFTER-FAILED');
  const heldByFailed = standing();
  assert.equal(heldByFailed.standing, 'version_taken', JSON.stringify(heldByFailed));
  assert.equal(heldByFailed.commit, fourth);
  assert.ok([firstRun, failedRun].includes(heldByFailed.run), JSON.stringify(heldByFailed));

  const ownRun = recorded(fourth, 'promoted');
  const republished = standing();
  assert.equal(republished.standing, 'published', `a run of this commit did not settle it: ${JSON.stringify(republished)}`);
  assert.equal(republished.commit, fourth);
  assert.equal(republished.run, ownRun);

  const garbledId = randomUUID();
  const garbled = join(store, 'runs', 'release-pipeline', garbledId, 'run.json');
  mkdirSync(dirname(garbled), { recursive: true });
  const garbledBody = { run_id: garbledId, product, version, state: 'failed' };
  writeFileSync(garbled, JSON.stringify(garbledBody, null, '  '));
  report.runs.push({ path: garbled, body: garbledBody });
  const refused = run(stadoBinary, ['release', 'newest', '--root', workspace, '--product', product, '--plan', '--json'], workspace);
  assert.ok(!exitedCleanly(refused), `a run without a commit was read: ${refused.stdout}`);
  assert.ok(refused.stderr.includes(`runs/release-pipeline/${garbledId}/run.json`), refused.stderr);
  assert.ok(refused.stderr.includes('names no source_commit'), refused.stderr);
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
