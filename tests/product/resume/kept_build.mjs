import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the build an install keeps when it fails after its
// build step, and of the next install resuming after that build. The journey
// runs the Stado under test with HOME set to an isolated directory under
// .build, so the runs it writes and the records it reads are never the
// person's; the workspace is the real one, read for the product's checkout.
// It installs `STADO_PRODUCT_TEST_PRODUCT` (a catalogued product with a
// stado-release service recipe) from STADO_PRODUCT_TEST_COMMIT, default the
// checkout's origin/main, signing with an empty keychain the journey creates
// (`WISENT_CODESIGN_KEYCHAIN`), so the source is resolved, the quality gates
// and the build pass, and signing refuses: the install fails after its build.
// Expected: the first install leaves its run whole and kept (`.kept`, the
// export's build output, `install-built/<key>.json`); the second install of
// the same commit says it resumed from that run and fails at signing again
// without a second run holding an export; the kept run is tagged as a cache.
// Required: STADO_BIN (the built Stado under test), STADO_PRODUCT_TEST_PRODUCT
// and STADO_PRODUCT_TEST_SOURCE (this machine's checkout of it); the host
// must be a Mac, since only a darwin build signs after it builds. A failed
// case is kept in the report and then ends the run with its error; a host
// that is not a Mac ends the run blocked, never passed.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const product = process.env.STADO_PRODUCT_TEST_PRODUCT;
const source = process.env.STADO_PRODUCT_TEST_SOURCE;
const stado = process.env.STADO_BIN;
assert.ok(stado && product && source,
  'STADO_BIN, STADO_PRODUCT_TEST_PRODUCT and STADO_PRODUCT_TEST_SOURCE are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-resume-kept-build-'));
const workspace = dirname(realpathSync(source));
const report = { started_at: new Date().toISOString(), product, source, workspace, cases: [], commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
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
function refusal(answer, why) {
  assert.ok(answer.status && !answer.error, why);
  return answer.stderr;
}
// The directories named `name` anywhere below the checkouts' build areas
// under `home` (`~/.stado/products/builds`), each with its entries.
function areas(home, name) {
  const found = [];
  const walk = directory => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const path = join(directory, entry.name);
      if (entry.name === name) {
        for (const child of readdirSync(path)) found.push(join(path, child));
        continue;
      }
      walk(path);
    }
  };
  const builds = join(home, '.stado', 'products', 'builds');
  if (existsSync(builds)) walk(builds);
  return found;
}
// The one element of `list`, refused with `why` when it holds none or more.
function only(list, why) {
  const [first, ...others] = list;
  assert.ok(first, `${why}: none found`);
  assert.deepEqual(others, [], `${why}: more than one found in ${JSON.stringify(list)}`);
  return first;
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  if (process.platform !== 'darwin') {
    report.result = 'blocked';
    report.blocked = `only a darwin build signs after it builds, and this host is ${process.platform}; the journey needs a Mac`;
    throw new Error(report.blocked);
  }
  const commit = process.env.STADO_PRODUCT_TEST_COMMIT
    || success(run('git', ['-C', source, 'rev-parse', 'origin/main']));
  report.product_commit = commit;
  const home = join(output, 'home');
  mkdirSync(join(home, '.stado'), { recursive: true });
  // A keychain that exists and holds no identity: the signing source resolves
  // before the build, and signing after the build refuses.
  const keychain = join(output, 'empty.keychain-db');
  success(run('security', ['create-keychain', '-p', 'journey', keychain]));
  const environment = { HOME: home, WISENT_CODESIGN_KEYCHAIN: keychain };
  const install = ['product', '--workspace', workspace, 'install', product, '--surface', 'service', '--source-commit', commit];

  // The first install builds, fails at signing, and keeps its build.
  const first = refusal(run(binary, install, environment), 'An install signing with an empty keychain must fail');
  assert.doesNotMatch(first, /resumed at signing/, 'The first install of a commit has nothing to resume from');
  const runs = areas(home, 'install');
  const kept = only(runs.filter(run => existsSync(join(run, '.kept'))), `a kept run among ${JSON.stringify(runs)} after: ${first}`);
  assert.ok(statSync(join(kept, 'source', '.wisent-output')).isDirectory(), 'The kept run lost its build output');
  assert.ok(existsSync(join(kept, 'CACHEDIR.TAG')), 'The kept run is not tagged as a cache');
  const record = JSON.parse(readFileSync(only(areas(home, 'install-built'), 'a kept-build record'), 'utf8'));
  assert.equal(realpathSync(record.evidence), realpathSync(kept), 'The record names another run than the kept one');
  assert.equal(record.revision, commit);
  report.cases.push({ case: 'kept', run: kept, record, refusal: first });

  // The second install resumes after the kept build and fails at signing
  // again, without exporting or building anew.
  const second = refusal(run(binary, install, environment), 'The resumed install signing with an empty keychain must fail');
  assert.match(second, /resumed at signing from the build kept at /, 'The second install did not say it resumed');
  assert.ok(second.includes(kept), `The second install resumed from another run than ${kept}: ${second}`);
  const exported = areas(home, 'install').filter(run => existsSync(join(run, 'source')));
  assert.deepEqual(exported.map(run => realpathSync(run)), [realpathSync(kept)],
    'The second install exported the source again instead of resuming');
  assert.ok(existsSync(join(kept, '.kept')), 'The resumed install released the kept run although it did not complete');
  report.cases.push({ case: 'resumed', refusal: second });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'A darwin source install that fails at signing keeps its build and the next install of the same commit resumes after it, in an isolated home; not a successful installation, product sync or a GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
