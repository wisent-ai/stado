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
// 4. A pass checks the step's main and installs its last green commit: the
//    state names the step delivered at that revision with no push, or
//    refused with its refusals and the defects recorded for them in Oko.
// 5. A second pass installs nothing again: a delivered step is said as
//    already installed, a refused one as waiting for a commit, and no defect
//    is recorded twice.
// 6. Register writes the register refuses end no pass. With a red record of
//    the last green commit's parent naming a defect recorded while Brama gave
//    no judgment (its repair needs a judge or an attestation, and the pass
//    runs with neither, as the routine does whenever Brama is down) and a
//    refused install of the step there naming a defect the register does not
//    hold, the next pass records the green commit green with its refused
//    repair, installs it, says both refused writes, keeps both repairs to
//    record again, ends cleanly, and `stado ci delivery` lists both under
//    `unrecorded`; a later pass with an attestation standing in for the
//    judge records the first repair and keeps the second. Without a green
//    commit or a first install the journey fails naming why.
// 7. `stado ci status` carries the plan and the state; the plan is cleared,
//    and clearing it again is refused.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const stadoBinary = realpathSync(required('STADO_BIN'));
const okoBinary = realpathSync(required('OKO_BIN'));
const step = required('STADO_DELIVERY_TEST_STEP');
// The open Oko defect the journey's installs repair: the operator's rulings
// are asked for it, so the journey runs only where they admit that purpose.
const purpose = required('STADO_DELIVERY_TEST_PURPOSE');
const [product, surface] = step.split(':');
const checkout = step.slice(`${product}:${surface}:`.length);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'ci-delivery-run', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
// The passes run with no attestation standing in for Brama's judgment, as
// the routine runs: a write that needs a judge Brama does not give is refused.
const inherited = { ...process.env };
delete inherited.OKO_ATTESTATION;
const environment = {
  ...inherited,
  PATH: `${dirname(okoBinary)}:${process.env.PATH}`,
  OKO_DEFECT_REGISTER: join(output, 'defects.json'),
  STADO_DELIVERY_PLAN: join(output, 'delivery-plan.json'),
  STADO_DELIVERY_STATE: join(output, 'delivery-state.json'),
  STADO_CI_MAIN_RECORD: join(output, 'ci-main.json'),
};
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  oko: okoBinary, oko_sha256: digest(readFileSync(okoBinary)),
  step, commands: [], verdict: 'failed',
};
function run(args, extra = {}) {
  const env = { ...environment, ...extra };
  const result = spawnSync(stadoBinary, args, { cwd: output, env, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}
function ok(args) {
  const result = run(args);
  assert.ok(exitedCleanly(result), `stado ${args.join(' ')} was refused: ${result.stderr}`);
  return result.stdout;
}
function refused(args, pattern) {
  const result = run(args);
  assert.ok(!exitedCleanly(result), `stado ${args.join(' ')} was accepted`);
  assert.match(result.stderr, pattern);
}
function oko(args, extra = {}) {
  const env = { ...environment, ...extra };
  const result = spawnSync(okoBinary, args, { cwd: output, env, encoding: 'utf8' });
  report.commands.push({ program: 'oko', args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  assert.ok(exitedCleanly(result), `oko ${args.join(' ')} was refused: ${result.stderr}`);
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

  refused(['ci', 'plan', 'set', '--purpose', purpose, '--step', `${key}:relative/checkout`], /names the checkout relative\/checkout relatively/);
  const empty = join(output, 'not-a-checkout');
  mkdirSync(empty);
  refused(['ci', 'plan', 'set', '--purpose', purpose, '--step', `${key}:${empty}`], /holds no git checkout/);
  refused(['ci', 'plan', 'set', '--purpose', purpose, '--step', step, '--step', step], /is named twice/);
  assert.ok(!existsSync(environment.STADO_DELIVERY_PLAN), 'a refused plan was written');

  const declared = JSON.parse(ok(['ci', 'plan', 'set', '--purpose', purpose, '--step', step, '--json']));
  assert.deepEqual(declared.steps, [{ product, surface, checkout }]);
  assert.equal(declared.purpose, purpose, 'the plan does not keep its purpose');
  assert.equal(declared.push, false, 'a plan declared without --push pushes');
  assert.deepEqual(JSON.parse(ok(['ci', 'plan', 'show', '--json'])).steps, declared.steps);

  const main = git(['rev-parse', '--verify', 'refs/heads/main']);
  const originBefore = git(['rev-parse', 'refs/remotes/origin/main']);
  const first = run(['ci', 'run', '--json']);
  const afterFirst = state();
  assert.equal(afterFirst.running, null, 'a finished pass left a step running');
  const checked = JSON.parse(readFileSync(environment.STADO_CI_MAIN_RECORD, 'utf8')).steps[key];
  const [latest] = [...checked].reverse();
  assert.equal(latest.revision, main, 'the main check did not check main HEAD');
  const green = checked.findLast(commit => commit.green);
  if (exitedCleanly(first) && green) {
    assert.equal(afterFirst.delivered[key].revision, green.revision, 'the routine installed other than the last green commit');
    assert.equal(afterFirst.refused, null);
    assert.deepEqual(afterFirst.pushed, {}, 'a plan declared without --push pushed');
    assert.ok(afterFirst.last_pass.said.includes('the plan declares no push'), afterFirst.last_pass.said);
  } else if (afterFirst.refused) {
    assert.equal(afterFirst.refused.step, key);
    assert.equal(afterFirst.refused.revision, green.revision);
    const [firstRefusal] = afterFirst.refused.refusals;
    assert.ok(firstRefusal, 'a refused step kept no refusal');
    const { defects, unrecorded } = afterFirst.refused;
    assert.ok(defects.length || unrecorded.length, 'a refused step recorded no defect and kept no refused write');
    const [firstLine] = firstRefusal.split('\n');
    for (const id of afterFirst.refused.defects) {
      const [shown] = oko(['defects', 'show', id, '--json']);
      assert.ok(shown.evidence.includes(firstLine), shown.evidence);
    }
  } else {
    assert.ok(!green, 'a step with a green commit was neither installed nor refused');
    assert.ok(afterFirst.last_pass.said.includes('no commit of main the main check found green'), afterFirst.last_pass.said);
  }
  const recorded = recordedIds();

  const second = run(['ci', 'run']);
  const said = state().last_pass.said;
  if (exitedCleanly(first) && green) {
    assert.ok(exitedCleanly(second), second.stderr);
    assert.ok(said.includes(`${key} already installed at ${green.revision}`), said);
  } else if (afterFirst.refused) {
    assert.match(said, /no newer commit of its main is green; waiting for a commit that repairs/);
  }
  assert.deepEqual(recordedIds(), recorded, 'a second pass recorded a defect');
  assert.equal(git(['rev-parse', 'refs/remotes/origin/main']), originBefore, 'the journey moved the checkout\'s origin');

  // Register writes the register refuses end no pass. The main check record
  // gains a red commit of the last green commit's parent naming a defect
  // recorded while Brama gave no judgment, and the routine's state a refusal
  // of the step at that parent naming a defect the register does not hold,
  // with the step no longer delivered. The next pass checks the green commit
  // again and records it green with the refused repair beside it, installs
  // it and says the install's refused repair beside the install, writes the
  // step delivered and ends cleanly. Without a green commit, or after a
  // first pass that did not install, there is nothing whose repair could be
  // refused: the journey fails naming the state that prevented it.
  const blocked = !green
    ? `the main check found no commit of main green: ${JSON.stringify(latest.refusals)}`
    : !exitedCleanly(first)
      ? `the first pass did not install ${green.revision}: ${first.stderr}`
      : null;
  assert.equal(blocked, null, `the refused-write legs cannot run: ${blocked}`);
  const parent = git(['rev-parse', `${green.revision}^`]);
  // Brama is unreachable for this one record: a loopback port the system
  // gave a listener that is closed again before Oko asks it.
  const listener = createServer();
  await new Promise(done => listener.listen(done));
  const brama = `http://localhost:${listener.address().port}`;
  await new Promise(done => listener.close(done));
  const unjudged = oko([
    'defects', 'record', '--product', product,
    '--title', `A red commit the delivery journey records while Brama gives no judgment (${randomUUID()})`,
    '--surface', 'stado ci run, the main check of the journey\'s step',
    '--evidence', 'Recorded by the journey into its own register with Brama unreachable',
    '--impact', 'None outside the journey: its register is a file of this run',
    '--repair', 'Nothing: the journey reads only that its repair record is refused',
    '--found-in', `${basename(checkout)}@${parent}`,
    '--criterion', 'the delivery journey ends',
    '--json',
  ], {
    STADO_MODEL_ROUTER_URL: brama,
    OKO_ATTESTATION: 'The delivery journey records this defect in its own register with Brama unreachable on purpose, so that recording its repair needs a judge.',
  }).id;
  const unheld = `a-defect-this-register-does-not-hold-${randomUUID()}`;
  const record = JSON.parse(readFileSync(environment.STADO_CI_MAIN_RECORD, 'utf8'));
  record.steps[key].push({
    revision: parent, at: new Date().toISOString(), green: false,
    refusals: ['a red commit the journey records'], defects: [unjudged],
  });
  writeFileSync(environment.STADO_CI_MAIN_RECORD, JSON.stringify(record));
  const seeded = state();
  delete seeded.delivered[key];
  seeded.refused = {
    step: key, revision: parent, refusals: ['an install the journey records refused'],
    defects: [unheld], at: new Date().toISOString(),
  };
  writeFileSync(environment.STADO_DELIVERY_STATE, JSON.stringify(seeded));
  const third = run(['ci', 'run', '--json']);
  assert.ok(exitedCleanly(third), `a refused register write ended the pass: ${third.stderr}`);
  const steps = JSON.parse(readFileSync(environment.STADO_CI_MAIN_RECORD, 'utf8')).steps[key];
  const again = steps.findLast(commit => commit.revision === green.revision);
  assert.equal(again.green, true, 'the green commit was recorded red because its repair record was refused');
  const [refusedByCheck, ...more] = again.unrecorded;
  assert.deepEqual(more, [], JSON.stringify(again));
  assert.ok(refusedByCheck.includes(`oko defects repaired ${unjudged}`), refusedByCheck);
  assert.match(refusedByCheck, /has no judgment/);
  assert.deepEqual(again.unrepaired, [unjudged], 'the refused repair was not kept to be recorded again');
  const afterThird = state();
  assert.equal(afterThird.delivered[key].revision, green.revision, 'the step was not delivered');
  assert.equal(afterThird.refused, null, 'the refusal stayed after the install');
  const [refusedByInstall, ...others] = afterThird.delivered[key].unrecorded;
  assert.deepEqual(others, [], JSON.stringify(afterThird.delivered[key]));
  assert.ok(refusedByInstall.includes(`oko defects repaired ${unheld}`), refusedByInstall);
  assert.deepEqual(afterThird.delivered[key].unrepaired, [unheld], JSON.stringify(afterThird.delivered[key]));
  const passSaid = afterThird.last_pass.said;
  assert.ok(passSaid.includes(`${key}: ${green.revision} green, its register writes refused`), passSaid);
  assert.ok(passSaid.includes(`${key}: its register write was refused`), passSaid);
  for (const defect of [unjudged, unheld]) {
    assert.ok(third.stderr.includes(`stado ci run: ${key} is`), third.stderr);
    assert.ok(third.stderr.includes(defect), `${defect} was not said on stderr`);
  }
  const delivery = JSON.parse(ok(['ci', 'delivery', '--json']));
  const listed = delivery.unrecorded.join('\n');
  assert.ok(listed.includes(`main of ${key} at "${green.revision}"`), listed);
  assert.ok(listed.includes(`step ${key} installed at ${green.revision}`), listed);
  for (const line of [refusedByCheck, refusedByInstall]) {
    assert.ok(listed.includes(line), `ci delivery does not list: ${line}`);
  }
  assert.match(ok(['ci', 'delivery']), /register write refused: /);

  // A later pass records the kept repairs again: with an attestation
  // standing in for the judge, the unjudged defect's repair is taken, and
  // the green commit keeps nothing to record; the unheld one stays kept.
  const fourth = run(['ci', 'run', '--json'], {
    OKO_ATTESTATION: 'The delivery journey attests that the green commit repairs the defect it recorded for its red parent.',
  });
  assert.ok(exitedCleanly(fourth), `the pass recording kept repairs was refused: ${fourth.stderr}`);
  const rechecked = JSON.parse(readFileSync(environment.STADO_CI_MAIN_RECORD, 'utf8')).steps[key];
  const repairedCommit = rechecked.findLast(commit => commit.revision === green.revision);
  assert.deepEqual(repairedCommit.unrepaired, [], JSON.stringify(repairedCommit));
  assert.deepEqual(repairedCommit.unrecorded, [], JSON.stringify(repairedCommit));
  const [shownRepaired] = oko(['defects', 'show', unjudged, '--json']);
  assert.ok(shownRepaired.repaired_in?.endsWith(`@${green.revision}`), JSON.stringify(shownRepaired));
  assert.deepEqual(state().delivered[key].unrepaired, [unheld], 'a repair the register still refuses was dropped');

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
