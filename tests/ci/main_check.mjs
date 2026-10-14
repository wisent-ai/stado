// Real journey of the main check through the built Stado and the built Oko.
// The plan, the routine's state, the main check record and Oko's defect
// register are files of this run under .build/ci-main-check, so the
// operator's are never read or written. The product is a Rust crate this
// journey commits into its own git checkout there, whose release manifest
// declares one lint gate: `cargo check --offline` over the committed tree.
//
// 1. A commit that compiles: one pass of `stado ci run` records it green, and
//    `stado ci delivery --json` names it the step's last green commit.
// 2. A commit that removes a function and a commit that still calls it: the
//    next pass checks both, oldest first; the first is green, the second red
//    with cargo's own error naming the file, line and the missing function,
//    and the red commit is recorded in Oko as a defect found in it. The last
//    green commit stays the earlier one, and `stado ci delivery` names the red
//    main as a way the delivery path is down.
// 3. A commit that repairs the call: the next pass records it green, the
//    earlier red defects carry it as their repair, and it is the last green.
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
// The open Oko defect the journey's pass serves: the operator's rulings are
// asked for it, so the journey runs only where they admit that purpose.
const purpose = required('STADO_DELIVERY_TEST_PURPOSE');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'ci-main-check', `run-${randomUUID()}`);
const product = 'ci-main-check-fixture';
const checkout = join(output, product);
mkdirSync(join(checkout, 'src'), { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = {
  ...process.env,
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
  commands: [], verdict: 'failed',
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
function oko(args) {
  const result = spawnSync(okoBinary, args, { cwd: output, env: environment, encoding: 'utf8' });
  report.commands.push({ program: 'oko', args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  assert.equal(result.status, successExit, `oko ${args.join(' ')} was refused: ${result.stderr}`);
  return JSON.parse(result.stdout);
}
function git(args) {
  const result = spawnSync('git', args, { cwd: checkout, encoding: 'utf8' });
  assert.equal(result.status, successExit, `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}
function commit(source, message) {
  writeFileSync(join(checkout, 'src', 'lib.rs'), source);
  git(['add', '--all']);
  git(['commit', '--quiet', '-m', message]);
  return git(['rev-parse', 'HEAD']);
}
const key = `${product}:cli`;
const main = () => JSON.parse(ok(['ci', 'delivery', '--json'])).routine.main.steps[key];

try {
  const own = JSON.parse(readFileSync(join(root, '.wisent-release.json'), 'utf8'));
  const [platform] = Object.keys(own.platforms);
  writeFileSync(join(checkout, 'Cargo.toml'),
    `[package]\nname = "${product}"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n`);
  writeFileSync(join(checkout, '.wisent-release.json'), JSON.stringify({
    schema_version: own.schema_version, product, releases: true,
    version_source: { kind: 'regex', path: 'Cargo.toml', pattern: '(?m)^version\\s*=\\s*"(?P<version>[^"]+)"\\s*$' },
    platforms: { [platform]: {
      runner_platform: platform,
      quality: [{ name: 'compiles', kind: 'lint', argv: ['cargo', 'check', '--offline', '--quiet'] }],
      build: { argv: ['cargo', 'build', '--offline'] },
      stage: {},
    } },
    promotion: { channels: ['candidate'], reconcile: false },
  }, null, '  '));
  git(['init', '--quiet', '--initial-branch=main']);
  const first = commit('pub fn kept() -> u8 { helper() }\npub fn helper() -> u8 { 7 }\n', 'compiles');
  ok(['ci', 'plan', 'set', '--purpose', purpose, '--step', `${key}:${checkout}`]);

  run(['ci', 'run']);
  let step = main();
  assert.equal(step.last_checked.revision, first);
  assert.equal(step.last_checked.green, true, JSON.stringify(step.last_checked));
  assert.equal(step.last_green.revision, first);

  const removed = commit('pub fn kept() -> u8 { 7 }\n', 'remove helper');
  const broken = commit('pub fn kept() -> u8 { 7 }\npub fn caller() -> u8 { helper() }\n', 'call the removed helper');
  run(['ci', 'run']);
  step = main();
  const [, second, third] = step.checked;
  assert.equal(second.revision, removed);
  assert.equal(second.green, true, JSON.stringify(second));
  assert.equal(third.revision, broken);
  assert.equal(third.green, false, 'the commit calling a removed function was not red');
  const [refusal] = third.refusals;
  assert.match(refusal, /helper/, refusal);
  assert.match(refusal, /src\/lib\.rs:\d+/, refusal);
  assert.ok(third.defects.length, 'the red commit recorded no defect in Oko');
  for (const id of third.defects) {
    const [shown] = oko(['defects', 'show', id, '--json']);
    assert.ok(JSON.stringify(shown).includes(broken), 'the defect does not name the red commit');
  }
  assert.equal(step.last_green.revision, removed, 'the last green commit moved past the red one');
  const stalled = JSON.parse(ok(['ci', 'delivery', '--json'])).stalled;
  assert.ok(stalled.some(line => line.includes(`main of ${key} is red at`)), stalled.join('\n'));

  const repaired = commit('pub fn kept() -> u8 { 7 }\npub fn caller() -> u8 { kept() }\n', 'call what exists');
  run(['ci', 'run']);
  step = main();
  assert.equal(step.last_checked.revision, repaired);
  assert.equal(step.last_checked.green, true, JSON.stringify(step.last_checked));
  assert.equal(step.last_green.revision, repaired);
  for (const id of third.defects) {
    const [shown] = oko(['defects', 'show', id, '--json']);
    assert.ok(JSON.stringify(shown).includes(repaired), 'the red defect does not carry the green commit as its repair');
  }
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
