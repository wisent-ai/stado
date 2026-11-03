import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of where a service surface built from source is placed.
// The Stado under test installs STADO_SERVICE_TEST_PRODUCT's service surface
// on STADO_SERVICE_TEST_HOST (this machine) from STADO_SERVICE_TEST_COMMIT,
// building it with its canonical recipe. Afterwards the catalog program the
// unit runs must be a regular executable under
// $HOME/.stado/services/<service>/current/<platform>, the receipt must name no
// file under ~/.stado/bin or ~/.local/bin, and the installation must not be
// refused for a launcher another product places under the same name in
// ~/.stado/bin (Weles' bin/start beside Skarbiec's and Brama's). Required:
// STADO_BIN, STADO_SERVICE_TEST_PRODUCT (a catalogued stado-release product
// with an installable service), STADO_SERVICE_TEST_HOST and
// STADO_SERVICE_TEST_COMMIT (a commit on its origin/main).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
const product = process.env.STADO_SERVICE_TEST_PRODUCT;
const host = process.env.STADO_SERVICE_TEST_HOST;
const commit = process.env.STADO_SERVICE_TEST_COMMIT;
assert.ok(stado && product && host && commit,
  'STADO_BIN, STADO_SERVICE_TEST_PRODUCT, STADO_SERVICE_TEST_HOST and STADO_SERVICE_TEST_COMMIT are required');
const home = process.env.HOME;
mkdirSync(join(root, '.build'), { recursive: true });
const reportPath = join(realpathSync(join(root, '.build')), `install-service-tree-${Date.now()}.json`);
const report = { started_at: new Date().toISOString(), stado, product, host, commit, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
report.stado_sha256 = digest(readFileSync(stado));

function run(args) {
  const answer = spawnSync(stado, args, { encoding: 'utf8', maxBuffer: 1 << 28 });
  report.commands.push({ args, status: answer.status, stderr_tail: (answer.stderr || '').slice(-4000) });
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  return answer;
}

try {
  const catalog = run(['service', 'catalog', '--json']);
  assert.equal(catalog.status, 0, catalog.stderr);
  const entry = JSON.parse(catalog.stdout).services.find(row => row.name === product);
  assert.ok(entry?.program, `the catalog declares no service program for ${product}`);
  const platform = { 'darwin-arm64': 'darwin-arm', 'linux-amd64': 'linux-amd' }[
    `${process.platform === 'darwin' ? 'darwin' : 'linux'}-${process.arch === 'arm64' ? 'arm64' : 'amd64'}`];
  const program = entry.program.replace('$HOME', home).replace('$STADO_PLATFORM', platform);
  report.program = program;

  const install = run(['product', 'install', product, '--surface', 'service', '--host', host,
    '--source-commit', commit, '--wait', '--json']);
  assert.doesNotMatch(install.stderr, /belongs to another recorded product/,
    'a service launcher must not be placed into the shared ~/.stado/bin');
  assert.equal(install.status, 0, install.stderr.slice(-2000));
  const receipt = JSON.parse(install.stdout);
  report.receipt_status = receipt.status;

  assert.ok(existsSync(program), `the catalog program ${program} was not placed`);
  const stat = lstatSync(program);
  assert.ok(stat.isFile() && (stat.mode & 0o111), `${program} is not a regular executable`);
  const shared = [join(home, '.stado/bin'), join(home, '.local/bin')];
  const placed = (receipt.files ?? []).map(file => file.path ?? '');
  for (const path of placed) {
    assert.ok(!shared.some(dir => path.startsWith(`${dir}/`)),
      `a service installation placed ${path} on the shared bin path`);
  }
  assert.ok(placed.some(path => path === program), `the receipt does not name ${program}`);
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  console.log(`report: ${reportPath} (${report.result})`);
}
