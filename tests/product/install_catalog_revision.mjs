import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of where an installation of Stado reads its
// `after_install` steps: from the catalog of the revision it installs, not
// from the catalog this command was given. The journey runs the Stado under
// test with HOME set to an isolated directory under .build and installs the
// `stado` CLI surface from STADO_PRODUCT_TEST_COMMIT (default the checkout's
// origin/main), handing the command a copy of the catalog whose `stado` CLI
// recipe declares one more after_install step naming a verb no Stado has.
// Expected: the install succeeds; its report's `catalog` provenance names
// the installed commit as the revision read, chosen as the installed
// revision; the steps it ran are the committed recipe's, and the verb the
// copy added was never run. Required: STADO_BIN (the built Stado under test)
// and
// STADO_PRODUCT_TEST_SOURCE (this machine's checkout of stado). The install
// builds Stado from source, so it takes as long as that build.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && source, 'STADO_BIN and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-install-catalog-revision-'));
const workspace = dirname(realpathSync(source));
const report = { started_at: new Date().toISOString(), source, workspace, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const UNKNOWN_VERB = 'verb-no-stado-has';
function run(program, args, environment) {
  const env = { ...process.env, ...environment };
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, environment, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const commit = process.env.STADO_PRODUCT_TEST_COMMIT
    || success(run('git', ['-C', source, 'rev-parse', 'origin/main']));
  report.commit = commit;

  // The catalog this command is given: the committed one with one step the
  // installed revision cannot run added to stado's CLI recipe, right after
  // that recipe's own `after_install:` line.
  const committed = success(run('git', ['-C', source, 'show', `${commit}:catalog/products.yml`]));
  const recipe = '  - surface: cli\n    kind: stado-release\n    repository: wisent-ai/stado';
  const marker = '    after_install:\n';
  assert.ok(committed.includes(recipe), 'the committed catalog declares the stado CLI recipe');
  const recipeAt = committed.indexOf(recipe);
  assert.ok(committed.slice(recipeAt).includes(marker), 'the committed stado CLI recipe declares after_install');
  const cut = committed.indexOf(marker, recipeAt) + marker.length;
  const given = `${committed.slice(undefined, cut)}    - [stado, ${UNKNOWN_VERB}]\n${committed.slice(cut)}`;
  const catalog = join(output, 'products.yml');
  writeFileSync(catalog, given);
  report.given_catalog = catalog;

  const home = join(output, 'home');
  mkdirSync(join(home, '.stado'), { recursive: true });
  const installed = JSON.parse(success(run(binary,
    ['product', '--workspace', workspace, '--catalog', catalog, 'install', 'stado', '--surface', 'cli', '--source-commit', commit],
    { HOME: home })));
  report.installed = installed;
  const where = installed.extra?.catalog ?? installed.catalog;
  assert.ok(where, 'the install report names the catalog it executed');
  assert.equal(where.revision, commit, 'the catalog was read at a revision other than the installed one');
  assert.equal(where.chosen, 'installed-revision', `the catalog was chosen as ${where.chosen}`);
  const outcomes = installed.extra?.after_install ?? installed.after_install;
  assert.ok(Array.isArray(outcomes) && outcomes.length, 'the install ran the committed after_install steps');
  assert.ok(!outcomes.some(step => step.argv?.includes(UNKNOWN_VERB)),
    `the install ran the step the given catalog added: ${JSON.stringify(outcomes)}`);
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = error.message;
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, undefined, '  '));
  console.error(`report: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
