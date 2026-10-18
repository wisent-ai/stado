// Real journey: the delivery routine asks the operator's rulings, for the
// purpose its plan declares, before it is declared or run. The built Stado
// asks the built Oko on PATH; the rulings are read, never written; the plan
// and the routine's state are files of this run under .build/ci-delivery-ruling.
// STADO_DELIVERY_TEST_STEP names one catalogued product's step as
// PRODUCT:SURFACE:CHECKOUT; STADO_DELIVERY_TEST_PURPOSE and
// STADO_DELIVERY_TEST_OTHER_PURPOSE name two open Oko defects, the first the
// one the operator's consent covers (CI/CD's repair), the second one it does
// not.
//
// Oko's own answer (`oko admit install --product P --purpose D`) is the
// expectation for each purpose:
// - refused: `stado ci plan set` is refused quoting that refusal and writes no
//   plan, and `stado ci run` over a plan already on disk is refused the same
//   way and writes no state;
// - allowed: the plan is declared, keeps its purpose, and is cleared again.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
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
const step = required('STADO_DELIVERY_TEST_STEP');
const purposes = [required('STADO_DELIVERY_TEST_PURPOSE'), required('STADO_DELIVERY_TEST_OTHER_PURPOSE')];
const [product, surface] = step.split(':');
const checkout = step.slice(`${product}:${surface}:`.length);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'ci-delivery-ruling', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = {
  ...process.env,
  PATH: `${dirname(okoBinary)}:${process.env.PATH}`,
  STADO_DELIVERY_PLAN: join(output, 'delivery-plan.json'),
  STADO_DELIVERY_STATE: join(output, 'delivery-state.json'),
};
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  oko: okoBinary, oko_sha256: digest(readFileSync(okoBinary)),
  step, purposes, answers: {}, commands: [], verdict: 'failed',
};
function spawn(program, args) {
  const result = spawnSync(program, args, { cwd: output, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}

function journey(purpose) {
  rmSync(environment.STADO_DELIVERY_PLAN, { force: true });
  rmSync(environment.STADO_DELIVERY_STATE, { force: true });
  const verdict = spawn(okoBinary, ['admit', 'install', '--product', product, '--purpose', purpose]);
  const allowed = exitedCleanly(verdict);
  report.answers[purpose] = { allowed, said: (verdict.stderr || verdict.stdout).trim() };
  const declare = ['ci', 'plan', 'set', '--purpose', purpose, '--step', step, '--json'];
  if (allowed) {
    const declared = spawn(stadoBinary, declare);
    assert.ok(exitedCleanly(declared), declared.stderr);
    assert.equal(JSON.parse(declared.stdout).purpose, purpose, 'the plan does not keep its purpose');
    const cleared = spawn(stadoBinary, ['ci', 'plan', 'clear']);
    assert.ok(exitedCleanly(cleared), cleared.stderr);
    return;
  }
  const ruling = report.answers[purpose].said;
  assert.ok(ruling, 'Oko refused without saying why');
  const declared = spawn(stadoBinary, declare);
  assert.ok(!exitedCleanly(declared), `a plan the rulings forbid for ${purpose} was declared`);
  assert.ok(declared.stderr.includes('declaring the delivery plan is refused'), declared.stderr);
  assert.ok(declared.stderr.includes(ruling), `the refusal does not quote Oko's: ${declared.stderr}`);
  assert.ok(!existsSync(environment.STADO_DELIVERY_PLAN), 'a refused plan was written');

  // A plan already on disk, declared before the ruling, is not run.
  writeFileSync(environment.STADO_DELIVERY_PLAN, JSON.stringify({
    steps: [{ product, surface, checkout }],
    push: false,
    purpose,
    declared_at: new Date().toISOString(),
  }));
  const pass = spawn(stadoBinary, ['ci', 'run']);
  assert.ok(!exitedCleanly(pass), `a pass the rulings forbid for ${purpose} ran`);
  assert.ok(pass.stderr.includes('stado ci run is refused'), pass.stderr);
  assert.ok(pass.stderr.includes(ruling), `the refusal does not quote Oko's: ${pass.stderr}`);
  assert.ok(!existsSync(environment.STADO_DELIVERY_STATE), 'a refused pass wrote the state');
}

try {
  for (const purpose of purposes) {
    journey(purpose);
  }
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
