import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado product install` when the workspace holds no
// canonical checkout of the product or of its git dependencies: the
// installation clones each GitHub origin into <workspace>/<name> on main,
// builds, signs and installs from them. A directory already at that path that
// is not the checkout is refused and left as it was.
//
// The workspace is a dedicated directory below this checkout's ignored .build;
// HOME stays the machine's own, because signing reads the vault through the
// bearer file there. The product must not be installed before the run: it is
// installed, checked, removed with `stado product remove` and checked absent,
// so the machine ends as it started. Required:
// STADO_PRODUCT_TEST_REPOSITORY_PRODUCT, a catalogued product with a public
// repository and a CLI surface that is not installed here. STADO_BIN selects
// the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const product = process.env.STADO_PRODUCT_TEST_REPOSITORY_PRODUCT;
assert.ok(product, 'STADO_PRODUCT_TEST_REPOSITORY_PRODUCT is required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-checkout-creation-'));
const stado = process.env.STADO_BIN || 'stado';
const report = { started_at: new Date().toISOString(), product, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args, env = process.env, cwd = root) {
  const answer = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.equal(answer.status, 0, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const workspaceOf = (name) => {
  const workspace = join(output, name);
  mkdirSync(workspace, { recursive: true });
  return { workspace, env: { ...process.env, WISENT_WORKSPACE: workspace } };
};
let binary;
let installedHere = false;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/product/src', 'tests/product']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const catalog = JSON.parse(success(run(binary, ['product', 'catalog', '--json'])));
  const record = (catalog.products || []).find(entry => entry.id === product);
  const recipe = (record?.installations || []).find(row => row.surface === 'cli');
  assert.ok(recipe?.repository, `${product} has no CLI recipe with a repository`);
  const name = recipe.repository.split('/').pop();
  const origin = `https://github.com/${recipe.repository}.git`;
  const before = JSON.parse(success(run(binary, ['product', 'status', product, '--surface', 'cli', '--json'])));
  assert.equal(before.status, 'absent', `${product}/cli is installed here; the run would replace the operator's installation`);

  const fresh = workspaceOf('fresh');
  assert.deepEqual(readdirSync(fresh.workspace), [], 'The dedicated workspace must start empty');
  const installed = run(binary, ['product', 'install', product, '--surface', 'cli', '--json'], fresh.env);
  installedHere = installed.status === 0;
  const checkout = join(fresh.workspace, name);
  assert.match(installed.stderr, new RegExp(`cloned ${origin.replace(/[.]/g, '\\.')} into `), 'The clone must be reported');
  assert.equal(success(run('git', ['remote', 'get-url', 'origin'], process.env, checkout)), origin);
  assert.equal(success(run('git', ['branch', '--show-current'], process.env, checkout)), 'main');
  success(installed);
  const state = JSON.parse(installed.stdout);
  report.installed = state;
  assert.equal(state.status, 'installed', `${product} was not installed from the created checkout`);
  assert.ok((state.installed_paths || []).length > 0, 'The installation placed nothing');
  for (const path of state.installed_paths) {
    assert.ok(existsSync(path), `${path} is recorded but not on disk`);
  }
  assert.equal(state.source_revision, success(run('git', ['rev-parse', 'HEAD'], process.env, checkout)),
    'The installation must be built from the created checkout');

  const removed = JSON.parse(success(run(binary, ['product', 'remove', product, '--surface', 'cli', '--json'], fresh.env)));
  installedHere = false;
  report.removed = removed;
  for (const path of state.installed_paths) {
    assert.equal(existsSync(path), false, `${path} is still on disk after remove`);
  }
  const after = JSON.parse(success(run(binary, ['product', 'status', product, '--surface', 'cli', '--json'])));
  assert.equal(after.status, 'absent', `${product}/cli is still recorded after remove`);

  const occupied = workspaceOf('occupied');
  const squatter = join(occupied.workspace, name);
  mkdirSync(squatter);
  writeFileSync(join(squatter, 'kept'), 'not a checkout\n');
  const refused = run(binary, ['product', 'install', product, '--surface', 'cli'], occupied.env);
  assert.notEqual(refused.status, 0, 'A directory that is not the checkout must be refused');
  assert.match(refused.stderr, /exists and does not identify/, 'The refusal must name the occupied path');
  assert.deepEqual(readdirSync(squatter), ['kept'], 'The occupying directory must be left as it was');
  assert.equal(existsSync(join(squatter, '.git')), false, 'Nothing may be cloned into an occupied path');
  const untouched = JSON.parse(success(run(binary, ['product', 'status', product, '--surface', 'cli', '--json'])));
  assert.equal(untouched.status, 'absent', 'A refused installation must install nothing');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  if (installedHere && binary) {
    report.cleanup = run(binary, ['product', 'remove', product, '--surface', 'cli', '--json']).status;
  }
  report.finished_at = new Date().toISOString();
  report.scope = 'Canonical checkout creation from the real GitHub origins, a real signed CLI installation and its removal; the occupied-path refusal';
  for (const name of ['fresh', 'occupied']) {
    rmSync(join(output, name), { recursive: true, force: true });
  }
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
