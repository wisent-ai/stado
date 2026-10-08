// Real read of the fleet's rollout policies through the built Stado and the
// operator's configured registry, read only: `release policy list` names every
// release-controlled product, `release policy show` returns each one as the
// `{product, policy}` document `release policy apply --file` takes and agrees
// with the list, an unknown product is refused naming the products that have a
// policy, and the retired policy-apply/policy-remove/policy-target-remove verbs
// are no commands. Nothing is written to the registry.
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
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'release-policy-read', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  binary, binary_sha256: digest(readFileSync(binary)), commands: [], verdict: 'failed',
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
  if (sentence) assert.ok(result.stderr.includes(sentence), `stado ${args.join(' ')}: ${result.stderr}`);
}

try {
  const listed = accepted(['release', 'policy', 'list', '--json']);
  assert.ok(Array.isArray(listed.products), 'policy list answered no products array');
  for (const row of listed.products) {
    const shown = accepted(['release', 'policy', 'show', row.product, '--json']);
    assert.deepEqual(Object.keys(shown).sort(), ['policy', 'product']);
    assert.equal(shown.product, row.product);
    assert.deepEqual(Object.keys(shown.policy.targets).sort(), [...row.targets].sort());
    assert.deepEqual(shown.policy.desired ?? null, row.desired ?? null);
  }
  const unknown = `no-such-product-${randomUUID()}`;
  refused(['release', 'policy', 'show', unknown], `${unknown} has no rollout policy; products with one:`);
  for (const retired of ['policy-apply', 'policy-remove', 'policy-target-remove']) refused(['release', retired]);
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
