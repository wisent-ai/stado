import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of how a catalog whose declarations do not hold is
// refused: as the declared state refusing (`config`), naming the product and
// what it declares wrongly, never as a failure nobody could attribute
// (`unknown`). The journey runs the Stado under test with HOME and the
// workspace isolated under .build, writes Stado's own committed catalog beside
// no checkout with its first product repeated, and reads it with
// `stado product catalog --json --catalog <file>`. Expected: a nonzero exit,
// stderr naming the duplicate product, and the failure classed `config`.
// Required: STADO_BIN, the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: the Stado executable under test');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-catalog-declaration-class-'));
const home = join(output, 'home');
const workspace = join(home, 'workspace');
const catalogPath = join(output, 'loose', 'products.yml');
const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: workspace, GIT_CONFIG_GLOBAL: '/dev/null' };
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const succeeded = answer => answer.status !== null && !answer.status;
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: isolated, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout;
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD'])).trim();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])).trim() };

  // Stado's own committed catalog, with its first product's record repeated
  // after itself: every field is one the validation accepts, and the only
  // declaration that fails is the product's id, declared twice.
  const committed = success(run('git', ['-C', root, 'show', 'HEAD:catalog/products.yml']));
  const lines = committed.split('\n');
  const starts = lines.flatMap((line, index) => (line.startsWith('- id: ') ? [index] : []));
  assert.ok(starts.length > 1, 'the committed catalog holds more than one product');
  const [first, second] = starts;
  const duplicated = lines[first].slice('- id: '.length);
  const repeated = [...lines.slice(0, second), ...lines.slice(first, second), ...lines.slice(second)];
  mkdirSync(dirname(catalogPath), { recursive: true });
  writeFileSync(catalogPath, repeated.join('\n'));
  report.duplicated = duplicated;

  const refused = run(binary, ['product', '--workspace', workspace, '--catalog', catalogPath, 'catalog', '--json']);
  assert.ok(!succeeded(refused), 'a catalog declaring one product twice was answered');
  assert.ok(refused.stderr.includes(`duplicate product ${duplicated}`),
    `the refusal does not name the duplicate product: ${refused.stderr}`);
  assert.match(refused.stderr, /\[config\]/, `the refusal is not classed config: ${refused.stderr}`);
  assert.doesNotMatch(refused.stderr, /\[unknown\]/, `the refusal is still unattributed: ${refused.stderr}`);
  report.refusal = refused.stderr;
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = error.stack || String(error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  rmSync(home, { recursive: true, force: true });
}
if (failure) throw failure;
