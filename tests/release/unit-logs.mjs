// Real read of a replace-strategy product's logs through the built Stado and
// the operator's configured registry, read only. `release policy list` names
// the release-controlled products; every one whose policy strategy is
// `replace` is the case: `release logs <product> --target <target> --json`
// must answer from the managed unit the policy names (`service`), with one
// unit record per declared unit carrying the file it read and its lines, for
// `--lines` as well as whole; `--head` is refused by name for such a product.
// A product with no replace target is no case; a run that finds none is
// blocked, never passed. Nothing is written on any host.
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
const binary = realpathSync(required('STADO_BIN'));
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const tailLines = required('STADO_TEST_UNIT_LOG_TAIL_LINES');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'release-unit-logs', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  binary, binary_sha256: digest(readFileSync(binary)), commands: [], cases: [], verdict: 'failed',
};
function stado(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}
function accepted(args) {
  const result = stado(args);
  assert.equal(result.status, successExit, `stado ${args.join(' ')} was refused: ${result.stderr}`);
  return JSON.parse(result.stdout);
}
function refused(args, sentence) {
  const result = stado(args);
  assert.notEqual(result.status, successExit, `stado ${args.join(' ')} was accepted`);
  assert.ok(result.stderr.includes(sentence), `stado ${args.join(' ')}: ${result.stderr}`);
}

try {
  const listed = accepted(['release', 'policy', 'list', '--json']);
  assert.ok(Array.isArray(listed.products), 'policy list answered no products array');
  for (const row of listed.products) {
    const shown = accepted(['release', 'policy', 'show', row.product, '--json']);
    if (shown.policy.strategy.kind !== 'replace') continue;
    for (const target of Object.keys(shown.policy.targets)) {
      report.cases.push({ product: row.product, target, service: shown.policy.service });
      const whole = accepted(['release', 'logs', row.product, '--target', target, '--json']);
      assert.equal(whole.product, row.product);
      assert.equal(whole.target, target);
      assert.equal(whole.service, shown.policy.service, 'the read names the service the policy names');
      assert.ok(whole.unit, 'the read names the managed unit that service runs as on the target');
      assert.equal(whole.selection, 'whole');
      assert.ok(Array.isArray(whole.units) && whole.units.length, 'no unit record was read');
      for (const unit of whole.units) {
        assert.equal(unit.host, target);
        assert.ok(unit.origin, 'a unit record names the file or journal it read');
        assert.ok(Array.isArray(unit.lines));
      }
      const tail = accepted(['release', 'logs', row.product, '--target', target, '--lines', tailLines, '--json']);
      assert.equal(tail.selection, 'tail');
      for (const unit of tail.units) assert.ok(unit.lines.length <= Number(tailLines), `tail longer than ${tailLines} lines`);
      refused(['release', 'logs', row.product, '--target', target, '--lines', tailLines, '--head'],
        `rolls out by replacing the managed unit its service ${shown.policy.service} runs as`);
    }
  }
  assert.ok(report.cases.length, 'blocked: no release-controlled product rolls out by replace');
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
