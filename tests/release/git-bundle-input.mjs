// Real Stado publication/readback and offline npm consumption of a Git package.
// No product build, private network credential, or operator store is used.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const binary = realpathSync(required('STADO_BIN'));
const source = realpathSync(required('STADO_TEST_GIT_SOURCE'));
const revision = required('STADO_TEST_GIT_REVISION');
const packageName = required('STADO_TEST_GIT_PACKAGE');
const packageFile = required('STADO_TEST_GIT_PACKAGE_FILE');
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const parent = join(root, '.build', 'git-bundle-input');
mkdirSync(parent, { recursive: true });
const output = mkdtempSync(join(parent, 'run-'));
const fixture = join(output, 'fixture');
const home = join(fixture, 'home');
const checkout = join(fixture, 'product');
const consumer = join(fixture, 'consumer');
const temporary = join(fixture, 'temporary');
for (const directory of [home, checkout, consumer, temporary]) mkdirSync(directory, { recursive: true });
const manifestPath = join(checkout, '.wisent-release.json');
const declaration = JSON.parse(readFileSync(join(root, '.wisent-release.json'), 'utf8'));
writeFileSync(manifestPath, JSON.stringify({ ...declaration, product: 'example-git-input', inputs: {} }));
const environment = {
  PATH: process.env.PATH, HOME: home, TMPDIR: temporary,
  STADO_CONFIG: join(home, 'config.json'), WC_STORAGE_BACKEND: 'local',
  WC_LOCAL_STORAGE_PATH: join(fixture, 'objects'),
  npm_config_cache: join(fixture, 'npm-cache'), GIT_ALLOW_PROTOCOL: 'file',
  GIT_CONFIG_NOSYSTEM: 'true', GIT_CONFIG_GLOBAL: join(home, 'gitconfig'),
};
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = { started_at: new Date().toISOString(), binary, binary_sha256: digest(readFileSync(binary)), commands: [], cases: [], verdict: 'failed' };
function command(program, args, cwd = root) {
  const directory = join(output, 'commands', randomUUID());
  mkdirSync(directory, { recursive: true });
  const stdout = join(directory, 'stdout');
  const stderr = join(directory, 'stderr');
  const out = openSync(stdout, 'w');
  const err = openSync(stderr, 'w');
  let result;
  try { result = spawnSync(program, args, { cwd, env: environment, stdio: ['ignore', out, err] }); }
  finally { closeSync(out); closeSync(err); }
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal, error: result.error?.message, stdout, stderr });
  return { ...result, stdout: readFileSync(stdout, 'utf8'), stderr: readFileSync(stderr, 'utf8') };
}
function success(program, args, cwd) {
  const result = command(program, args, cwd);
  assert.equal(result.status, successExit, `${program} ${args.join(' ')}: ${result.stderr}`);
  return result.stdout;
}
const refs = () => success('git', ['for-each-ref', '--format=%(refname) %(objectname)'], source);
const pin = ['release', 'catalog', 'pin-input', checkout, '--name', 'example-git-bundle', '--source', source, '--revision', revision, '--git-bundle', '--json'];
function refusal(name, args, expected) {
  const before = readFileSync(manifestPath);
  const beforeRefs = refs();
  const result = command(binary, args);
  assert.ok(result.status !== null && result.status !== successExit, `${name} was accepted`);
  assert.ok(result.stderr.includes(expected), `${name}: ${result.stderr}`);
  assert.deepEqual(readFileSync(manifestPath), before);
  assert.equal(refs(), beforeRefs);
  report.cases.push({ name, verdict: 'passed', status: result.status, manifest_unchanged: true, source_refs_unchanged: true });
}
try {
  report.source_revision = success('git', ['rev-parse', 'HEAD']).trim();
  report.source_diff = success('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/release_catalog/publisher/input.rs', 'stado-rs/src/cli/release_catalog/publisher/input', 'tests/release/git-bundle-input.mjs']);
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.binary_version = success(binary, ['--version']).trim();
  report.bundle_export_sha256 = digest(readFileSync(join(root, 'stado-rs/src/cli/release_catalog/publisher/input/bundle.rs')));
  report.npm_version = success('npm', ['--version']).trim();
  report.input_revision = success('git', ['rev-parse', '--verify', `${revision}^{commit}`], source).trim();
  const beforeRefs = refs();
  const beforeStatus = success('git', ['status', '--porcelain'], source);
  const receipt = JSON.parse(success(binary, pin));
  const saved = JSON.parse(readFileSync(manifestPath, 'utf8'));
  assert.equal(receipt.source_commit, report.input_revision);
  assert.deepEqual(saved.inputs['example-git-bundle'], { uri: receipt.uri, sha256: receipt.sha256, mount: 'example-git-bundle.bundle', extract: false });
  const bundle = join(fixture, 'source.bundle');
  success(binary, ['storage', 'get', receipt.uri, bundle]);
  assert.equal(digest(readFileSync(bundle)), receipt.sha256);
  success('git', ['bundle', 'verify', bundle]);
  const heads = success('git', ['bundle', 'list-heads', bundle]).trim().split('\n');
  assert.deepEqual(heads.map(line => line.split(' ').shift()), [report.input_revision]);
  assert.equal(refs(), beforeRefs);
  assert.equal(success('git', ['status', '--porcelain'], source), beforeStatus);
  report.cases.push({ name: 'publish-readback-and-source-preservation', verdict: 'passed', receipt, persisted_manifest: saved });
  writeFileSync(join(consumer, 'package.json'), JSON.stringify({ private: true, dependencies: { [packageName]: `git+${pathToFileURL(bundle).href}#${report.input_revision}` } }));
  success('npm', ['install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund'], consumer);
  const expected = success('git', ['show', `${report.input_revision}:${packageFile}`], source);
  assert.equal(readFileSync(join(consumer, 'node_modules', packageName, packageFile), 'utf8'), expected);
  report.cases.push({ name: 'offline-npm-consumes-locked-package', verdict: 'passed', package: packageName, file: packageFile, sha256: digest(expected) });
  refusal('bundle-path-conflict', [...pin, '--path', packageFile], '--git-bundle cannot be combined');
  refusal('bundle-cargo-conflict', [...pin, '--cargo'], '--git-bundle cannot be combined');
  refusal('unknown-revision', pin.map(value => value === revision ? `refs/heads/example-absent-${randomUUID()}` : value), 'rev-parse');
  refusal('missing-source', pin.map(value => value === source ? join(fixture, 'absent-source') : value), 'git');
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error);
} finally {
  report.finished_at = new Date().toISOString();
  rmSync(fixture, { recursive: true, force: true });
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') throw new Error(report.error);
