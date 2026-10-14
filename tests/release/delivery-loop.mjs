// Real journey of the delivery loop through the built Stado and the built
// Oko. A workspace of its own under .build/release-delivery-loop holds one
// product checkout, a real Git repository with a release manifest of the
// schema Stado's own manifest declares, committed under the Git identity this
// machine declares; Oko's defect register (OKO_DEFECT_REGISTER) and the
// loop's register (STADO_RELEASE_LOOP_REGISTER) are files of this run, so the
// operator's registers are never touched. Stado's recorded release runs are
// read, not written.
//
// 1. A commit no open defect's repair is in owes nothing: `stado release
//    loop run` keeps it as nothing_carried and exits successfully.
// 2. A defect whose recorded repair is that commit is carried: the loop
//    hands the commit to the release pipeline, which refuses the fixture
//    product; the register keeps the commit failed at the release step with
//    that refusal, the command exits unsuccessfully naming the product, and
//    the defect carries a note naming the commit, the step and the error.
// 3. The same commit is not taken again: a second pass leaves its entry as
//    it was and writes no second note.
// 4. `stado release loop status` reads the same entries.
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
const output = join(root, '.build', 'release-delivery-loop', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const product = `loop-fixture-${randomUUID()}`;
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
const git = args => ok('git', args, checkout).trim();
const loopRun = () => run(stadoBinary, ['release', 'loop', 'run', '--root', workspace, '--channel', 'candidate', '--json'], workspace);
const register = () => JSON.parse(readFileSync(environment.STADO_RELEASE_LOOP_REGISTER, 'utf8')).entries;
const entryOf = commit => register().find(entry => entry.product === product && entry.commit === commit);
function defect(id) {
  const [shown] = JSON.parse(ok(okoBinary, ['defects', 'show', id, '--json'], workspace));
  return shown;
}

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
  const commit = git(['rev-parse', 'HEAD']);

  const nothing = loopRun();
  assert.equal(nothing.status, successExit, `a commit carrying nothing was refused: ${nothing.stderr}`);
  assert.equal(entryOf(commit).state, 'nothing_carried');

  const recorded = ok(okoBinary, [
    'defects', 'record', '--product', product, '--title', `delivery loop fixture ${product}`,
    '--surface', 'release loop', '--evidence', 'tests/release/delivery-loop.mjs',
    '--impact', 'none: test record', '--repair', 'the fixture commit', '--found-in', 'delivery-loop-test', '--json',
  ], workspace);
  const id = JSON.parse(recorded).id;
  ok(okoBinary, ['defects', 'repaired', id, '--revision', `${product}@${commit}`, '--detail', 'the fixture commit'], checkout);

  const refused = loopRun();
  assert.notEqual(refused.status, successExit, 'a carried commit the pipeline refuses was accepted');
  assert.ok(refused.stderr.includes(`did not reach a release: ${product}`), refused.stderr);
  const failed = entryOf(commit);
  assert.equal(failed.state, 'failed');
  assert.equal(failed.step, 'release');
  assert.deepEqual(failed.carried, [id]);
  assert.ok(failed.error, 'the failed entry keeps no error');
  const notes = defect(id).notes;
  const sentence = `stado release loop: ${product} at ${commit} did not reach a release; the release step refused: ${failed.error}`;
  assert.deepEqual(notes.filter(note => note.text === sentence).map(note => note.text), [sentence]);

  loopRun();
  assert.deepEqual(entryOf(commit), failed, 'a commit already failed was taken again');
  assert.deepEqual(defect(id).notes, notes, 'a second pass noted the defect again');

  const status = JSON.parse(ok(stadoBinary, ['release', 'loop', 'status', '--product', product, '--json'], workspace));
  assert.deepEqual(status.entries, [failed]);
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
