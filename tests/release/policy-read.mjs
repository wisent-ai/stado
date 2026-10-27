// Real read of the fleet's rollout policies through the built Stado and the
// operator's configured registry, read only: `release policy list` names every
// release-controlled product, `release policy show` returns each one as the
// `{product, policy}` document `release policy apply --file` takes and agrees
// with the list, an unknown product is refused naming the products that have a
// policy, a stored policy whose install root lies under one target's home is
// refused when re-applied (naming its {home} form, before anything is read or
// written) and `release policy root repair --check` names that same {home}
// form without writing it (a root already under {home} is reported as such),
// and the retired policy-apply/policy-remove/policy-target-remove verbs are no
// commands. Nothing is written to the registry.
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
const binary = realpathSync(required('STADO_BIN'));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(root, '.build', 'release-policy-read', `run-${randomUUID()}`);
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  binary, binary_sha256: digest(readFileSync(binary)), commands: [], home_bound_refused: [], verdict: 'failed',
};
function stado(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  return result;
}
function accepted(args) {
  const result = stado(args);
  assert.ok(exitedCleanly(result), `stado ${args.join(' ')} was refused: ${result.stderr}`);
  return JSON.parse(result.stdout);
}
function refused(args, sentence) {
  const result = stado(args);
  assert.ok(!exitedCleanly(result), `stado ${args.join(' ')} was accepted`);
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
    assert.deepEqual(shown.policy.desired, row.desired);
    // A stored root under one target's home while another target's home
    // differs is refused when re-applied, before the registry is read, so
    // this read-only test writes nothing; the refusal names the {home} form.
    const homes = Object.values(shown.policy.targets).map(target => target.home);
    const owner = homes.find(home => shown.policy.install_root.startsWith(`${home}/`));
    if (!shown.policy.install_root.includes('{home}') && owner && homes.some(home => home !== owner)) {
      const { desired, previous, ...policy } = shown.policy;
      const file = join(output, `${row.product}-policy.json`);
      writeFileSync(file, JSON.stringify({ product: row.product, policy }));
      const rest = shown.policy.install_root.slice(owner.length);
      refused(['release', 'policy', 'apply', '--file', file], `write it as {home}${rest}`);
      const check = accepted(['release', 'policy', 'root', 'repair', row.product, '--check', '--json']);
      assert.equal(check.status, 'would_rewrite');
      assert.equal(check.portable_root, `{home}${rest}`);
      assert.equal(accepted(['release', 'policy', 'show', row.product, '--json']).policy.install_root, shown.policy.install_root);
      report.home_bound_refused.push(row.product);
    } else if (shown.policy.install_root.includes('{home}')) {
      const check = accepted(['release', 'policy', 'root', 'repair', row.product, '--check', '--json']);
      assert.equal(check.status, 'already_portable');
    }
  }
  const unknown = `no-such-product-${randomUUID()}`;
  refused(['release', 'policy', 'show', unknown], `${unknown} has no rollout policy; products with one:`);
  refused(['release', 'policy', 'root', 'repair', unknown, '--check'], `${unknown} has no rollout policy; products with one:`);
  for (const retired of ['policy-apply', 'policy-remove', 'policy-target-remove']) refused(['release', retired]);
  for (const retired of ['remove-target', 'portable-root']) refused(['release', 'policy', retired, unknown]);
  report.verdict = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
