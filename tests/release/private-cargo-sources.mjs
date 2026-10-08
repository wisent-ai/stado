// Real immutable publication and Cargo resolution, without compiling a product.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const binary = realpathSync(required('STADO_BIN'));
const source = realpathSync(required('STADO_TEST_PRIVATE_SOURCE'));
const revision = required('STADO_TEST_PRIVATE_REVISION');
const cargoHome = realpathSync(required('CARGO_HOME'));
const rustupHome = realpathSync(required('RUSTUP_HOME'));
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const parent = join(root, '.build', 'private-cargo-sources');
mkdirSync(parent, { recursive: true });
const output = mkdtempSync(join(parent, 'run-'));
const fixture = join(output, 'fixture');
const home = join(fixture, 'home');
const checkout = join(fixture, 'product');
const manifestPath = join(checkout, '.wisent-release.json');
const sourceManifest = JSON.parse(readFileSync(join(source, '.wisent-release.json'), 'utf8'));
for (const directory of [home, checkout]) mkdirSync(directory, { recursive: true });
writeFileSync(manifestPath, JSON.stringify({ ...sourceManifest, product: 'example-cargo-input' }));
const environment = {
  ...process.env, HOME: home, CARGO_HOME: cargoHome, RUSTUP_HOME: rustupHome,
  STADO_CONFIG: join(home, 'config.json'), WC_STORAGE_BACKEND: 'local',
  WC_LOCAL_STORAGE_PATH: join(fixture, 'objects'), WISENT_OUTPUT_DIR: join(fixture, 'output'),
};
delete environment.WISENT_SOURCE_DIR;
delete environment.WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR;
const report = { started_at: new Date().toISOString(), binary, binary_sha256: digest(readFileSync(binary)), commands: [], cases: [], verdict: 'failed' };
function digest(bytes) { return createHash('sha256').update(bytes).digest('hex'); }
function command(program, args, env = environment, cwd = root) {
  const directory = join(output, 'commands', randomUUID());
  mkdirSync(directory, { recursive: true });
  const stdout = join(directory, 'stdout');
  const stderr = join(directory, 'stderr');
  const out = openSync(stdout, 'w');
  const err = openSync(stderr, 'w');
  let result;
  try { result = spawnSync(program, args, { cwd, env, stdio: ['ignore', out, err] }); }
  finally { closeSync(out); closeSync(err); }
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal, error: result.error?.message, stdout, stderr });
  return { ...result, stdout: readFileSync(stdout, 'utf8'), stderr: readFileSync(stderr, 'utf8') };
}
function success(program, args, env, cwd) {
  const result = command(program, args, env, cwd);
  assert.equal(result.status, successExit, `${program} ${args.join(' ')}: ${result.stderr}`);
  return result;
}
const pin = ['release', 'catalog', 'pin-input', checkout, '--name', 'private-cargo-sources', '--source', source, '--revision', revision, '--cargo', '--json'];
const lockPath = join(source, 'Cargo.lock');
const lockDigest = digest(readFileSync(lockPath));
function refusal(name, args, env = environment) {
  const before = readFileSync(manifestPath);
  const result = command(binary, args, env);
  assert.ok(result.status !== null && result.status !== successExit, `${name} was accepted`);
  assert.deepEqual(readFileSync(manifestPath), before, `${name} changed the manifest`);
  assert.equal(digest(readFileSync(lockPath)), lockDigest, `${name} changed Cargo.lock`);
  report.cases.push({ name, verdict: 'passed', status: result.status, manifest_unchanged: true, lock_unchanged: true });
}
try {
  report.source_revision = success('git', ['rev-parse', 'HEAD']).stdout.trim();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.binary_version = success(binary, ['--version']).stdout.trim();
  report.input_source_revision = success('git', ['rev-parse', '--verify', `${revision}^{commit}`], environment, source).stdout.trim();
  const before = readFileSync(manifestPath);
  const receipt = JSON.parse(success(binary, pin).stdout);
  const saved = JSON.parse(readFileSync(manifestPath, 'utf8'));
  assert.deepEqual(saved.inputs['private-cargo-sources'], { uri: receipt.uri, sha256: receipt.sha256, mount: 'private-cargo-sources', extract: true });
  assert.notDeepEqual(readFileSync(manifestPath), before);
  const archive = join(fixture, 'sources.tar.gz');
  success(binary, ['storage', 'get', receipt.uri, archive]);
  assert.equal(digest(readFileSync(archive)), receipt.sha256);
  const repeated = JSON.parse(success(binary, pin).stdout);
  assert.equal(repeated.sha256, receipt.sha256);
  assert.equal(repeated.uri, receipt.uri);
  report.cases.push({ name: 'immutable-export-and-repeat', verdict: 'passed', receipt, persisted_manifest: saved });
  refusal('cargo-path-conflict', [...pin, '--path', 'Cargo.lock']);
  refusal('wrong-input-name', pin.map(value => value === 'private-cargo-sources' ? 'example-wrong-input' : value));
  const input = join(fixture, 'input');
  const workerCargo = join(fixture, 'worker-cargo');
  mkdirSync(input);
  mkdirSync(workerCargo);
  symlinkSync(join(cargoHome, 'registry'), join(workerCargo, 'registry'), 'dir');
  success('tar', ['-xzf', archive, '-C', input]);
  const provenancePath = join(input, 'provenance.json');
  const provenanceBytes = readFileSync(provenancePath);
  const provenance = JSON.parse(provenanceBytes);
  const worker = { ...environment, CARGO_HOME: workerCargo, WISENT_SOURCE_DIR: source, WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR: input, GIT_ALLOW_PROTOCOL: '', CARGO_NET_GIT_FETCH_WITH_CLI: 'true' };
  const metadataArgs = ['product', 'cargo', '--manifest-path', join(source, 'Cargo.toml'), 'metadata', '--format-version=1'];
  const metadata = JSON.parse(success(binary, metadataArgs, worker).stdout);
  for (const expected of provenance.packages) {
    const actual = metadata.packages.find(pkg => pkg.name === expected.name && pkg.version === expected.version && pkg.source === expected.source);
    assert.ok(actual, `Cargo omitted ${expected.name} ${expected.version} from ${expected.source}`);
    assert.ok(realpathSync(actual.manifest_path).startsWith(`${realpathSync(join(input, 'sources'))}${sep}`), `Cargo did not consume the mounted source: ${actual.manifest_path}`);
  }
  assert.equal(existsSync(join(workerCargo, 'git')), false, 'worker created a Git cache');
  assert.equal(digest(readFileSync(lockPath)), lockDigest);
  report.cases.push({ name: 'real-cargo-without-private-git-cache', verdict: 'passed', packages: provenance.packages, lock_unchanged: true });
  const absent = { ...worker };
  delete absent.WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR;
  refusal('missing-private-input', metadataArgs, absent);
  writeFileSync(provenancePath, JSON.stringify({ ...provenance, packages: [] }));
  refusal('mismatched-private-packages', metadataArgs, worker);
  writeFileSync(provenancePath, provenanceBytes);
  const [first] = provenance.packages;
  assert.ok(first, 'qualification source must lock a private Git crate');
  const packagePath = join(input, 'sources', `${first.name}-${first.version}`);
  const retained = join(fixture, 'held-package');
  renameSync(packagePath, retained);
  try { refusal('missing-private-package', metadataArgs, worker); }
  finally { renameSync(retained, packagePath); }
  assert.equal(digest(readFileSync(lockPath)), lockDigest);
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack ?? error);
} finally {
  report.finished_at = new Date().toISOString();
  rmSync(fixture, { recursive: true, force: true });
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') throw new Error(report.error);
