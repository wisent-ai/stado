// Real journey of the delivery queue that alone moves a product's `main`,
// through the built Stado and the built Oko. A run of its own under
// .build/ci-delivery-queue holds a bare origin and one product checkout whose
// release manifest takes Stado's own platform recipes and replaces their
// quality gates with a format gate that passes and a lint gate that prints
// the committed VIOLATION file as clippy prints a lint and refuses while the
// file exists. The checkout's pre-push hook is `stado quality check
// --pre-push`; Oko's defect register and the queue's register are files of
// this run.
//
// 1. A push of the violating commit to `main` is refused by the pre-push
//    check, naming the delivery queue; the origin's `main` does not move.
// 2. The same commit pushed to the delivery branch passes the compile-free
//    checks; `stado ci deliver` gates it, refuses it with the lint's own
//    line among its refusals, leaves the origin's `main` where it was, and
//    records one Oko defect carrying that line, owned by the session whose
//    recorded repair rides on the commit.
// 3. A second pass takes the refused candidate as decided: no new run and
//    no second defect.
// 4. The repair pushed to the delivery branch passes every gate: the origin's
//    `main` moves to it, and the refused candidate's defect records it as
//    its repair.
// Needs STADO_BIN, OKO_BIN and STADO_TEST_SUCCESS_EXIT.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
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
const output = join(root, '.build', 'ci-delivery-queue', `run-${randomUUID()}`);
const origin = join(output, 'origin.git');
const product = `delivery-fixture-${randomUUID()}`;
const checkout = join(output, 'workspace', product);
const author = randomUUID();
mkdirSync(checkout, { recursive: true });
mkdirSync(origin, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = {
  ...process.env,
  PATH: `${dirname(okoBinary)}:${process.env.PATH}`,
  OKO_DEFECT_REGISTER: join(output, 'defects.json'),
  STADO_DELIVERY_QUEUE_REGISTER: join(output, 'delivery-queue.json'),
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
const originMain = () => ok('git', ['rev-parse', 'refs/heads/main'], origin).trim();
const deliver = () => run(stadoBinary, ['ci', 'deliver', '--checkout', checkout, '--json'], checkout);
const register = () => JSON.parse(readFileSync(environment.STADO_DELIVERY_QUEUE_REGISTER, 'utf8')).entries;
function defect(id) {
  const [shown] = JSON.parse(ok(okoBinary, ['defects', 'show', id, '--json'], checkout));
  return shown;
}
const productDefects = () => JSON.parse(ok(okoBinary, ['defects', 'list', '--product', product, '--json'], checkout));

try {
  const lint = 'src/lib.rs:16:5: error: match expression looks like `matches!` macro (clippy::match_like_matches_macro)';
  const passingFormat = { name: 'passing-format', argv: ['sh', '-c', 'exit'], kind: 'format' };
  const lintGate = {
    name: 'violation-lint',
    argv: ['sh', '-c', 'if test -e VIOLATION; then cat VIOLATION >&2; exit 3; fi'],
    kind: 'lint',
  };
  const platforms = Object.fromEntries(Object.entries(own.platforms)
    .map(([name, recipe]) => [name, { ...recipe, quality: [passingFormat, lintGate] }]));
  ok('git', ['init', '-q', '--bare', '-b', 'main'], origin);
  git(['init', '-q', '-b', 'main']);
  git(['remote', 'add', 'origin', origin]);
  writeFileSync(join(checkout, 'VERSION'), `${own.schema_version}.${own.schema_version}.${own.schema_version}\n`);
  writeFileSync(join(checkout, '.wisent-release.json'), JSON.stringify({
    schema_version: own.schema_version, product, releases: true,
    version_source: { kind: 'text', path: 'VERSION' },
    platforms, promotion: { channels: ['candidate'], reconcile: false }, deliveries: [],
  }, null, '  '));
  git(['add', '.']);
  git(['commit', '-q', '-m', 'fixture']);
  const seed = git(['rev-parse', 'HEAD']);
  git(['push', '-q', 'origin', 'HEAD:main']);
  const hook = join(checkout, '.git', 'hooks', 'pre-push');
  writeFileSync(hook, `#!/bin/sh\nexec '${stadoBinary}' quality check --pre-push\n`);
  chmodSync(hook, '755');

  writeFileSync(join(checkout, 'VIOLATION'), `${lint}\n`);
  git(['add', 'VIOLATION']);
  git(['commit', '-q', '-m', 'violation']);
  const violating = git(['rev-parse', 'HEAD']);
  const carried = JSON.parse(ok(okoBinary, [
    'defects', 'record', '--product', product, '--title', `delivery queue fixture repair ${product}`,
    '--surface', 'delivery queue', '--evidence', 'tests/ci/delivery_queue.mjs',
    '--impact', 'none: test record', '--repair', 'the violating commit', '--found-in', `${product}@${seed}`, '--json',
  ], checkout)).id;
  ok(okoBinary, ['defects', 'repaired', carried, '--revision', `${product}@${violating}`, '--detail', 'the violating commit'], checkout);
  ok(okoBinary, ['defects', 'assign', carried, '--owner', author, '--priority', 'high'], checkout);

  // 1. main is not pushed to.
  const direct = run('git', ['push', 'origin', 'HEAD:main'], checkout);
  assert.notEqual(direct.status, successExit, 'a push to main of an unadmitted commit was taken');
  assert.match(direct.stderr + direct.stdout, /advances only through the\s+delivery queue/);
  assert.equal(originMain(), seed, 'the refused push moved main');

  // 2. The delivery branch takes it; the builder refuses it with every refusal.
  git(['push', '-q', 'origin', 'HEAD:delivery']);
  const refusedRun = deliver();
  assert.notEqual(refusedRun.status, successExit, 'a refused candidate exited as a pass');
  const refused = JSON.parse(refusedRun.stdout).entry;
  assert.equal(refused.state, 'refused');
  assert.equal(refused.candidate, violating);
  assert.equal(refused.main, seed);
  assert.ok(refused.refusals.some(refusal => refusal.includes('violation-lint') && refusal.includes(lint)), JSON.stringify(refused.refusals));
  assert.deepEqual(refused.authors, [author]);
  assert.equal(originMain(), seed, 'a refused candidate moved main');
  const recorded = defect(refused.defect);
  assert.ok(recorded.evidence.includes(lint), recorded.evidence);
  assert.equal(recorded.owner, author);

  // 3. A refused candidate is decided: no second run, no second defect.
  const again = deliver();
  assert.notEqual(again.status, successExit);
  assert.deepEqual(JSON.parse(again.stdout).entry, refused, 'a refused candidate was gated again');
  assert.deepEqual(
    productDefects().filter(listed => listed.title.includes(violating)).map(listed => listed.id),
    [recorded.id],
    'the refused candidate was recorded more than once',
  );

  // 4. The repair passes every gate and moves main.
  git(['rm', '-q', 'VIOLATION']);
  git(['commit', '-q', '-m', 'repair']);
  const repair = git(['rev-parse', 'HEAD']);
  git(['push', '-q', 'origin', 'HEAD:delivery']);
  const admitted = JSON.parse(ok(stadoBinary, ['ci', 'deliver', '--checkout', checkout, '--json'], checkout)).entry;
  assert.equal(admitted.state, 'admitted');
  assert.equal(admitted.candidate, repair);
  assert.deepEqual(admitted.refusals, []);
  assert.equal(originMain(), repair, 'an admitted candidate did not move main');
  const settled = register().find(entry => entry.candidate === violating);
  assert.equal(settled.repaired_by, repair);
  assert.equal(defect(refused.defect).repaired_in, `${product}@${repair}`);

  // Nothing waits once main holds the delivery tip.
  assert.equal(JSON.parse(ok(stadoBinary, ['ci', 'deliver', '--checkout', checkout, '--json'], checkout)).entry, null);
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
