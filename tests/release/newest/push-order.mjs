// Real journey of the delivery pass's push phase, through the built Stado
// and the built Oko. A workspace of its own under .build/release-push-order
// holds three product checkouts: one with commits its bare upstream (also a
// directory of this run) does not have, one even with its upstream, and one
// with no upstream at all. The queue's and Oko's registers are files of
// this run; nothing is built or pushed.
//
// 1. `stado release newest --plan --json` counts the unpushed commits of
//    every checkout: the ahead checkout's exact count, zero for the even
//    one, and for the one without an upstream no count, with git's reason.
// 2. `stado release newest --push` without `--first` is refused before any
//    step runs, naming why: the push waits for the products every push needs
//    first, and nothing names them. Its upstream is unchanged.
//
// Needs STADO_BIN and OKO_BIN.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, realpathSync, symlinkSync, writeFileSync } from 'node:fs';
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
const output = join(root, '.build', 'release-push-order', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const remotes = join(output, 'remotes');
const home = join(output, 'home');
mkdirSync(home, { recursive: true });
const bin = join(home, '.stado', 'bin');
mkdirSync(bin, { recursive: true });
symlinkSync(okoBinary, join(bin, 'oko'));
const runId = randomUUID();
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = {
  ...process.env,
  HOME: home,
  STADO_CONFIG: join(output, 'config.json'),
  WC_STORAGE_BACKEND: 'local',
  WC_LOCAL_STORAGE_PATH: join(output, 'store'),
  GIT_AUTHOR_NAME: 'Release journey',
  GIT_AUTHOR_EMAIL: 'journey@example.invalid',
  GIT_COMMITTER_NAME: 'Release journey',
  GIT_COMMITTER_EMAIL: 'journey@example.invalid',
  PATH: `${dirname(okoBinary)}:${process.env.PATH}`,
  OKO_DEFECT_REGISTER: join(output, 'defects.json'),
  STADO_RELEASE_LOOP_REGISTER: join(output, 'release-loop.json'),
};
writeFileSync(environment.STADO_CONFIG, '{}\n');
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
  assert.ok(exitedCleanly(result), `${program} ${args.join(' ')} was refused: ${result.stderr}`);
  return result.stdout;
}
function commit(directory, file) {
  writeFileSync(join(directory, file), `${file}\n`);
  ok('git', ['add', file], directory);
  ok('git', ['commit', '-q', '-m', file], directory);
}
function checkout(product, upstream) {
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
  if (upstream) {
    const bare = join(remotes, `${product}.git`);
    ok('git', ['init', '-q', '--bare', bare], output);
    ok('git', ['remote', 'add', 'origin', bare], directory);
    ok('git', ['push', '-q', '-u', 'origin', 'main'], directory);
  }
  return directory;
}

try {
  mkdirSync(remotes, { recursive: true });
  const ahead = checkout(`push-ahead-${runId}`, true);
  const even = checkout(`push-even-${runId}`, true);
  const loose = checkout(`push-loose-${runId}`, false);
  writeFileSync(join(workspace, 'notes.txt'), 'Not a checkout.\n');
  symlinkSync('missing-checkout', join(workspace, 'dangling'));
  symlinkSync(ahead, join(workspace, 'checkout-link'));
  const added = [randomUUID(), randomUUID()];
  for (const file of added) commit(ahead, file);
  const upstreamHead = ok('git', ['rev-parse', 'origin/main'], ahead).trim();

  const plan = JSON.parse(ok(stadoBinary, ['release', 'newest', '--root', workspace, '--plan', '--json'], workspace));
  const counted = Object.fromEntries(plan.unpushed.map(entry => [entry.repository, entry]));
  assert.deepEqual(Object.keys(counted).sort(),
    [ahead, even, loose].map(path => path.split('/').pop()).sort());
  const name = path => path.split('/').pop();
  assert.equal(counted[name(ahead)].ahead, added.length, JSON.stringify(plan.unpushed));
  assert.equal(counted[name(even)].ahead, added.length - added.length, JSON.stringify(plan.unpushed));
  assert.equal(counted[name(loose)].ahead, null, JSON.stringify(plan.unpushed));
  assert.ok(counted[name(loose)].unreadable.includes('git rev-list --count @{upstream}..HEAD exited'),
    counted[name(loose)].unreadable);

  const refused = run(stadoBinary, ['release', 'newest', '--root', workspace, '--push'], workspace);
  assert.ok(!exitedCleanly(refused), '--push without --first was accepted');
  assert.ok(refused.stderr.includes('--push pushes only after the --first products are delivered and installed'),
    refused.stderr);
  assert.equal(ok('git', ['rev-parse', 'origin/main'], ahead).trim(), upstreamHead, 'the refused pass pushed');
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
