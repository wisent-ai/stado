// Real journey of the delivery routine through the built Stado and the built
// Oko. The plan, the routine's state and Oko's defect register are files of
// this run under .build/ci-delivery-run, so the operator's plan and state are
// never read or written. STADO_DELIVERY_TEST_STEP names one catalogued
// product's step as PRODUCT:SURFACE:CHECKOUT, its checkout the canonical one.
// The plan is declared without --push, so the journey publishes nothing: the
// checkout's view of its origin is the same before and after.
//
// 1. A pass with no plan is refused naming `stado ci plan set`.
// 2. A relative checkout, a checkout that holds no git checkout and a step
//    named twice are refused, and no plan is written.
// 3. The plan is declared without --push and read back.
// 4. A pass installs the step from its checkout's main HEAD: the state names
//    the step delivered at that revision with no push, or refused with its
//    refusals and the defects recorded for them in Oko.
// 5. A second pass installs nothing again: a delivered step is said as
//    already installed, a refused one as waiting for a commit, and no defect
//    is recorded twice.
// 6. `stado ci status` carries the plan and the state; the plan is cleared,
//    and clearing it again is refused.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
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
const step = required('STADO_DELIVERY_TEST_STEP');
const [product, surface] = step.split(':');
const checkout = step.slice(`${product}:${surface}:`.length);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'ci-delivery-run', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = {
  ...process.env,
  PATH: `${dirname(okoBinary)}:${process.env.PATH}`,
  OKO_DEFECT_REGISTER: join(output, 'defects.json'),
  STADO_DELIVERY_PLAN: join(output, 'delivery-plan.json'),
  STADO_DELIVERY_STATE: join(output, 'delivery-state.json'),
};
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  oko: okoBinary, oko_sha256: digest(readFileSync(okoBinary)),
  step, commands: [], verdict: 'failed',
};
function run(args) {
  const result = spawnSync(stadoBinary, args, { cwd: output, env: environment, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}
function ok(args) {
  const result = run(args);
  assert.equal(result.status, successExit, `stado ${args.join(' ')} was refused: ${result.stderr}`);
  return result.stdout;
}
function refused(args, pattern) {
  const result = run(args);
  assert.notEqual(result.status, successExit, `stado ${args.join(' ')} was accepted`);
  assert.match(result.stderr, pattern);
}
function oko(args) {
  const result = spawnSync(okoBinary, args, { cwd: output, env: environment, encoding: 'utf8' });
  report.commands.push({ program: 'oko', args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  assert.equal(result.status, successExit, `oko ${args.join(' ')} was refused: ${result.stderr}`);
  return JSON.parse(result.stdout);
}
const git = args => spawnSync('git', args, { cwd: checkout, encoding: 'utf8' }).stdout.trim();
const state = () => JSON.parse(readFileSync(environment.STADO_DELIVERY_STATE, 'utf8'));
const recordedIds = () => existsSync(environment.OKO_DEFECT_REGISTER)
  ? oko(['defects', 'list', '--product', product, '--json']).map(defect => defect.id)
  : [];
const key = `${product}:${surface}`;

try {
  refused(['ci', 'run'], /no delivery plan is declared at .*stado ci plan set/);

  refused(['ci', 'plan', 'set', '--step', `${key}:relative/checkout`], /names the checkout relative\/checkout relatively/);
  const empty = join(output, 'not-a-checkout');
  mkdirSync(empty);
  refused(['ci', 'plan', 'set', '--step', `${key}:${empty}`], /holds no git checkout/);
  refused(['ci', 'plan', 'set', '--step', step, '--step', step], /is named twice/);
  assert.ok(!existsSync(environment.STADO_DELIVERY_PLAN), 'a refused plan was written');

  const declared = JSON.parse(ok(['ci', 'plan', 'set', '--step', step, '--json']));
  assert.deepEqual(declared.steps, [{ product, surface, checkout }]);
  assert.equal(declared.push, false, 'a plan declared without --push pushes');
  assert.deepEqual(JSON.parse(ok(['ci', 'plan', 'show', '--json'])).steps, declared.steps);

  const main = git(['rev-parse', '--verify', 'refs/heads/main']);
  const originBefore = git(['rev-parse', 'refs/remotes/origin/main']);
  const first = run(['ci', 'run', '--json']);
  const afterFirst = state();
  assert.equal(afterFirst.running, null, 'a finished pass left a step running');
  if (first.status === successExit) {
    assert.equal(afterFirst.delivered[key].revision, main);
    assert.equal(afterFirst.refused, null);
    assert.deepEqual(afterFirst.pushed, {}, 'a plan declared without --push pushed');
    assert.ok(afterFirst.last_pass.said.includes('the plan declares no push'), afterFirst.last_pass.said);
  } else {
    assert.equal(afterFirst.refused.step, key);
    assert.equal(afterFirst.refused.revision, main);
    const [firstRefusal] = afterFirst.refused.refusals;
    assert.ok(firstRefusal, 'a refused step kept no refusal');
    assert.ok(afterFirst.refused.defects.length, 'a refused step recorded no defect');
    const [firstLine] = firstRefusal.split('\n');
    for (const id of afterFirst.refused.defects) {
      const [shown] = oko(['defects', 'show', id, '--json']);
      assert.ok(shown.evidence.includes(firstLine), shown.evidence);
    }
  }
  const recorded = recordedIds();

  const second = run(['ci', 'run']);
  const said = state().last_pass.said;
  if (first.status === successExit) {
    assert.equal(second.status, successExit, second.stderr);
    assert.ok(said.includes(`${key} already installed at ${main}`), said);
  } else {
    assert.match(said, /has not moved; waiting for a commit that repairs/);
  }
  assert.deepEqual(recordedIds(), recorded, 'a second pass recorded a defect');
  assert.equal(git(['rev-parse', 'refs/remotes/origin/main']), originBefore, 'the journey moved the checkout\'s origin');

  const status = JSON.parse(ok(['ci', 'status', product, '--json']));
  assert.deepEqual(status.delivery.plan.steps, declared.steps);
  assert.equal(status.delivery.running, null);

  ok(['ci', 'plan', 'clear']);
  refused(['ci', 'plan', 'clear'], /no delivery plan is declared/);
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
