// Real journey of the delivery queue's compile-free gate step through the
// built Stado and the built Oko. A workspace of its own under
// .build/release-delivery-queue-gates holds one product checkout whose
// release manifest takes Stado's own platform recipes and replaces their
// quality gates with one format gate that refuses (`sh -c 'exit 3'`); Oko's
// defect register and the queue's register are files of this run.
//
// 1. A defect whose recorded repair is the checkout's commit makes the
//    commit owe a release; `stado release newest --plan` runs the format
//    gate before queueing, and the gate refuses: the register keeps the
//    commit failed at step `gates` with the gate's refusal, nothing is
//    built, and the defect carries the note naming the commit and step.
// 2. The commit is not taken again: a second `--plan` leaves the entry as it
//    was, and the gated `stado release newest` does not submit it.
//
// Needs STADO_BIN and OKO_BIN.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
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
const output = join(root, '.build', 'release-delivery-queue-gates', `run-${randomUUID()}`);
const workspace = join(output, 'workspace');
const product = `gates-fixture-${randomUUID()}`;
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
  assert.ok(exitedCleanly(result), `${program} ${args.join(' ')} was refused: ${result.stderr}`);
  return result.stdout;
}
const git = args => ok('git', args, checkout).trim();
const newest = ['release', 'newest', '--root', workspace, '--product', product, '--json'];
const plan = () => JSON.parse(ok(stadoBinary, [...newest, '--plan'], workspace));
const entryOf = commit => JSON.parse(readFileSync(environment.STADO_RELEASE_LOOP_REGISTER, 'utf8'))
  .entries.find(entry => entry.product === product && entry.commit === commit);
function defect(id) {
  const [shown] = JSON.parse(ok(okoBinary, ['defects', 'show', id, '--json'], workspace));
  return shown;
}

try {
  const refusingGate = { name: 'refusing-format', argv: ['sh', '-c', 'exit 3'], kind: 'format' };
  const platforms = Object.fromEntries(Object.entries(own.platforms)
    .map(([name, recipe]) => [name, { ...recipe, quality: [refusingGate] }]));
  git(['init', '-q', '-b', 'main']);
  writeFileSync(join(checkout, 'VERSION'), `${own.schema_version}.${own.schema_version}.${own.schema_version}\n`);
  writeFileSync(join(checkout, '.wisent-release.json'), JSON.stringify({
    schema_version: own.schema_version, product, releases: true,
    version_source: { kind: 'text', path: 'VERSION' },
    platforms, promotion: { channels: ['candidate'], reconcile: false }, deliveries: [],
  }, null, '  '));
  git(['add', '.']);
  git(['commit', '-q', '-m', 'fixture']);
  const commit = git(['rev-parse', 'HEAD']);

  const recorded = ok(okoBinary, [
    'defects', 'record', '--product', product, '--title', `delivery queue gates fixture ${product}`,
    '--surface', 'release queue', '--evidence', 'tests/release/delivery-queue-gates.mjs',
    '--impact', 'none: test record', '--repair', 'the fixture commit', '--found-in', 'delivery-queue-gates-test', '--json',
  ], workspace);
  const id = JSON.parse(recorded).id;
  ok(okoBinary, ['defects', 'repaired', id, '--revision', `${product}@${commit}`, '--detail', 'the fixture commit'], checkout);

  const [refused] = plan().queue;
  assert.equal(refused.state, 'failed');
  assert.equal(refused.step, 'gates');
  assert.ok(refused.error.includes('refusing-format'), refused.error);
  assert.deepEqual(refused.carried, [id]);
  assert.equal(git(['rev-parse', 'HEAD']), commit, 'the gate step committed');
  const sentence = `stado release loop: ${product} at ${commit} did not reach a release; the gates step refused: ${refused.error}`;
  const notes = defect(id).notes;
  assert.deepEqual(notes.filter(note => note.text === sentence).map(note => note.text), [sentence]);

  plan();
  assert.deepEqual(entryOf(commit), refused, 'a commit the gates refused was taken again');
  const delivered = run(stadoBinary, [...newest, '--channel', 'candidate'], workspace);
  const answer = JSON.parse(delivered.stdout);
  assert.deepEqual(answer.outcomes, [], 'release newest submitted a commit its gates refused');
  assert.deepEqual(defect(id).notes, notes, 'a later pass noted the defect again');
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
