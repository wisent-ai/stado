// Real journey: a quality check asks the operator's rulings before a lint
// gate compiles the tree, a test gate runs its tests, or the command tree's
// CLI is built. The built Stado asks the built Oko on PATH; the rulings are
// read, never written. STADO_QUALITY_TEST_CHECKOUT names a product checkout
// whose declared gates include a lint or test gate for this host,
// STADO_QUALITY_TEST_PRODUCT its product, and STADO_QUALITY_TEST_PURPOSE an
// open Oko defect the check serves.
//
// Oko's own answers (`oko admit compile|test|build --product P --purpose D`)
// are the expectation:
// - refused: `stado quality check --purpose D` exits non-zero quoting Oko's
//   refusal and naming the gate that was not run, and `stado quality
//   command-tree --purpose D` exits non-zero quoting it and saying the CLI
//   was not built;
// - allowed: neither names a gate the rulings kept from running nor a CLI
//   they kept from being built (a gate may still refuse on its own merit).
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
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
const checkout = realpathSync(required('STADO_QUALITY_TEST_CHECKOUT'));
const product = required('STADO_QUALITY_TEST_PRODUCT');
const purpose = required('STADO_QUALITY_TEST_PURPOSE');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const output = join(root, '.build', 'quality-admitted', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = { ...process.env, PATH: `${dirname(okoBinary)}:${process.env.PATH}` };
const revision = spawnSync('git', ['-C', checkout, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).stdout.trim();
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary, stado_sha256: digest(readFileSync(stadoBinary)),
  oko: okoBinary, oko_sha256: digest(readFileSync(okoBinary)),
  checkout, checked_revision: revision, product, purpose, answers: {}, commands: [], verdict: 'failed',
};
function spawn(program, args) {
  const result = spawnSync(program, args, { cwd: output, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}
function answer(action) {
  const verdict = spawn(okoBinary, ['admit', action, '--product', product, '--purpose', purpose]);
  const said = (verdict.stderr || verdict.stdout).trim();
  report.answers[action] = { allowed: exitedCleanly(verdict), said };
  return report.answers[action];
}

try {
  assert.ok(revision, `${checkout} names no committed revision`);
  const compile = answer('compile');
  const test = answer('test');
  const build = answer('build');

  const checked = spawn(stadoBinary, ['quality', 'check', '--root', checkout, '--revision', revision,
    '--purpose', purpose, '--json']);
  const checkSaid = `${checked.stdout}\n${checked.stderr}`;
  const gateRefusal = [compile, test].find(answered => !answered.allowed);
  if (gateRefusal) {
    assert.ok(gateRefusal.said, 'Oko refused without saying why');
    assert.ok(!exitedCleanly(checked), `a check whose gates the rulings forbid passed: ${checkSaid}`);
    assert.ok(checkSaid.includes('was not run'), `the refused gate is not named: ${checkSaid}`);
    assert.ok(checkSaid.includes("the operator's rulings do not allow it now"), checkSaid);
  } else {
    assert.ok(!checkSaid.includes('was not run'), `an admitted gate was kept from running: ${checkSaid}`);
  }

  const read = spawn(stadoBinary, ['quality', 'command-tree', '--root', checkout, '--revision', revision,
    '--purpose', purpose, '--json']);
  const readSaid = `${read.stdout}\n${read.stderr}`;
  if (!build.allowed) {
    assert.ok(build.said, 'Oko refused without saying why');
    assert.ok(!exitedCleanly(read), `a CLI the rulings forbid building was read: ${readSaid}`);
    assert.ok(readSaid.includes('was not built or read'), `the refused build is not said: ${readSaid}`);
  } else {
    assert.ok(!readSaid.includes('was not built or read'), `an admitted build was refused: ${readSaid}`);
  }
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
