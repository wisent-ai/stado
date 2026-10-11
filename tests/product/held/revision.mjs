import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the hold on a revision whose `stado serve` stops
// raised a defect: a source install of that revision is refused before
// anything is exported, and an unreadable stop file refuses it too. The
// journey runs the Stado under test with HOME set to an isolated directory
// under .build, so the stops it writes and the record it reads are never the
// person's; the workspace is the real one, read for the product's checkout.
// Each case installs `STADO_PRODUCT_TEST_PRODUCT` (a catalogued product with
// a stado-release service recipe) from STADO_PRODUCT_TEST_COMMIT, default
// the checkout's origin/main, and then reads the isolated record back:
// nothing is installed. Required: STADO_BIN (the built Stado under test),
// STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE (this machine's
// checkout of it). A failed case is kept in the report and then ends the
// run with its error.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && product && source,
  'STADO_BIN, STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-held-revision-'));
const workspace = dirname(realpathSync(source));
const report = { started_at: new Date().toISOString(), product, source, workspace, cases: [], commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args, home) {
  const env = home ? { ...process.env, HOME: home } : process.env;
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, home, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function refusal(answer, why) {
  assert.ok(answer.status && !answer.error, why);
  return answer.stderr;
}
function isolatedHome(name) {
  const home = join(output, name);
  mkdirSync(join(home, '.stado'), { recursive: true });
  return home;
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const commit = process.env.STADO_PRODUCT_TEST_COMMIT
    || success(run('git', ['-C', source, 'rev-parse', 'origin/main']));
  report.product_commit = commit;
  const install = ['product', '--workspace', workspace, 'install', product, '--surface', 'service', '--source-commit', commit];
  const status = ['product', '--workspace', workspace, 'status', product, '--surface', 'service', '--json'];

  // A stop of a build of this commit that raised its defect holds it.
  const held = isolatedHome('held');
  const stops = [
    { version: 'under-test', revision: commit, at: new Date().toISOString(), failure: 'the first stop', reported: false },
    { version: 'under-test', revision: commit, at: new Date().toISOString(), failure: 'the second stop', reported: true, report: 'recorded: the journey defect' },
  ];
  writeFileSync(join(held, '.stado', 'serve-stops.jsonl'), stops.map(stop => `${JSON.stringify(stop)}\n`).join(''));
  const before = JSON.parse(success(run(binary, status, held)));
  const refused = refusal(run(binary, install, held), 'A held revision must not be installed');
  assert.match(refused, new RegExp(`revision ${commit} is held: stado serve under-test built from it stopped ${stops.length} time\\(s\\)`));
  assert.match(refused, /the second stop; recorded: the journey defect/, 'The refusal must name the latest failure and the defect');
  const after = JSON.parse(success(run(binary, status, held)));
  assert.equal(after.source_revision, before.source_revision, 'A held install changed the record');
  report.cases.push({ case: 'held', refusal: refused });

  // A stop file that cannot be read refuses rather than building past it.
  const unreadable = isolatedHome('unreadable');
  mkdirSync(join(unreadable, '.stado', 'serve-stops.jsonl'));
  const blind = refusal(run(binary, install, unreadable), 'An unreadable stop file must refuse the install');
  assert.match(blind, new RegExp(`whether revision ${commit} is held cannot be told: .*serve-stops\\.jsonl cannot be read`));
  report.cases.push({ case: 'unreadable', refusal: blind });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'The source-install refusal of a held revision and of an unreadable stop file, in isolated homes; not a crash loop of stado serve itself, product sync or a GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
