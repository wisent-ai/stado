import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isolatedPath, pinnedGit, toolchainHomes } from '../../native/home.mjs';

// Real qualification of a CLI surface that carries a SwiftPM resource bundle
// (`<Package>_<Target>.bundle`, which Bundle.module finds beside the
// executable). The Stado under test installs STADO_BUNDLE_TEST_PRODUCT's CLI
// surface from STADO_BUNDLE_TEST_COMMIT with its canonical recipe into a HOME
// of its own under this checkout's ignored .build, so the person's installed
// CLI, its PATH links and its receipts are never touched. The installation
// must pass its readiness check; the bundle must be a directory under that
// home's ~/.stado/bin with no link on its ~/.local/bin; and `stado product
// status` must report the surface ready. Before the repair the readiness
// check judged the bundle as an executable and rolled every such
// installation back with '<bundle> is not executable'. Required: STADO_BIN,
// STADO_BUNDLE_TEST_PRODUCT (a catalogued product whose CLI stage places a
// resource directory under bin/) and STADO_BUNDLE_TEST_COMMIT (a commit on its
// origin/main). The workspace is the one holding this checkout, whose
// canonical checkouts the build reads.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
const product = process.env.STADO_BUNDLE_TEST_PRODUCT;
const commit = process.env.STADO_BUNDLE_TEST_COMMIT;
assert.ok(stado && product && commit,
  'STADO_BIN, STADO_BUNDLE_TEST_PRODUCT and STADO_BUNDLE_TEST_COMMIT are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-cli-bundle-'));
const home = join(output, 'home');
const workspace = dirname(root);
for (const directory of [join(home, '.stado'), join(home, '.local/bin'), join(output, 'tmp')]) {
  mkdirSync(directory, { recursive: true });
}
// The person's own install roots never reach the run: a program the product
// places under a name the person already has would otherwise be found there.
const personal = new Set([join(homedir(), '.local/bin'), join(homedir(), '.stado/bin')]);
const path = isolatedPath(home).split(delimiter).filter((entry) => !personal.has(entry)).join(delimiter);
const environment = {
  ...process.env, ...pinnedGit, ...toolchainHomes,
  HOME: home, PATH: path, TMPDIR: join(output, 'tmp'),
  STADO_CONFIG: join(home, '.stado/config.json'),
};
const reportPath = join(output, 'report.json');
const report = { started_at: new Date().toISOString(), stado, product, commit, home, commands: [], result: 'failed' };
report.stado_sha256 = createHash('sha256').update(readFileSync(stado)).digest('hex');

function run(args) {
  const answer = spawnSync(stado, args, { cwd: root, env: environment, encoding: 'utf8', maxBuffer: 1 << 28 });
  report.commands.push({ args, status: answer.status, stderr_tail: (answer.stderr || '').slice(-4000) });
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  return answer;
}

try {
  const initialised = run(['config', 'init']);
  assert.equal(initialised.status, 0, `config init refused: ${initialised.stderr}`);
  const installed = run(['product', '--workspace', workspace, 'install', product, '--surface', 'cli',
    '--source-commit', commit, '--json']);
  assert.equal(installed.status, 0, `install refused: ${installed.stderr}`);
  const receipt = JSON.parse(readFileSync(join(home, '.stado/products', product, 'cli.json'), 'utf8'));
  const paths = receipt.installed_paths ?? [];
  const bundles = paths.filter((placed) => placed.startsWith(join(home, '.stado/bin/')) && placed.endsWith('.bundle'));
  assert.ok(bundles.length > 0, `the receipt places no resource bundle under the isolated ~/.stado/bin: ${JSON.stringify(paths)}`);
  for (const bundle of bundles) {
    assert.ok(lstatSync(bundle).isDirectory(), `${bundle} is not a directory`);
    const name = bundle.slice(bundle.lastIndexOf('/') + 1);
    assert.ok(!paths.includes(join(home, '.local/bin', name)), `${name} was linked onto PATH`);
  }
  assert.ok(paths.every((placed) => placed.startsWith(home)), `a path outside the isolated home was installed: ${JSON.stringify(paths)}`);
  report.bundles = bundles;

  // Read right after the installation, so its revision is origin/main's and
  // nothing but a path fault could keep it from ready; a fault is 'drifted'.
  const status = run(['product', '--workspace', workspace, 'status', product, '--surface', 'cli', '--json']);
  assert.equal(status.status, 0, `status refused: ${status.stderr}`);
  const answer = JSON.parse(status.stdout);
  assert.notEqual(answer.status, 'drifted', `a path fault holds the surface: ${JSON.stringify(answer.readiness)}`);
  assert.doesNotMatch(JSON.stringify(answer.readiness), /\.bundle is not executable/);
  assert.equal(answer.readiness?.ready, true, `the surface is not ready: ${JSON.stringify(answer.readiness)}`);
  report.readiness = answer.readiness;
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  console.error(`report: ${reportPath}`);
  if (report.result === 'passed') rmSync(home, { recursive: true, force: true });
}
