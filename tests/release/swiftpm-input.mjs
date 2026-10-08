// Publish real locked Swift dependencies, restore them, and let SwiftPM consume them.
// This journey does not compile, sign, install, or use the operator's object store.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, createReadStream, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
}
const binary = realpathSync(required('STADO_BIN'));
const source = realpathSync(required('STADO_TEST_SWIFT_SOURCE'));
const revision = required('STADO_TEST_SWIFT_REVISION');
const successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
assert.ok(Number.isInteger(successExit));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const parent = join(root, '.build', 'swiftpm-input');
mkdirSync(parent, { recursive: true });
const output = mkdtempSync(join(parent, 'run-'));
const fixture = join(output, 'fixture');
const home = join(fixture, 'home');
const checkout = join(fixture, 'product');
const restored = join(fixture, 'restored');
const temporary = join(fixture, 'temporary');
for (const directory of [home, checkout, restored, temporary]) mkdirSync(directory, { recursive: true });
const manifestPath = join(checkout, '.wisent-release.json');
const declaration = JSON.parse(readFileSync(join(root, '.wisent-release.json'), 'utf8'));
writeFileSync(manifestPath, JSON.stringify({ ...declaration, product: 'example-swift-input', inputs: {} }));
const environment = {
  PATH: process.env.PATH, HOME: home, TMPDIR: temporary,
  STADO_CONFIG: join(home, 'config.json'), WC_STORAGE_BACKEND: 'local',
  WC_LOCAL_STORAGE_PATH: join(fixture, 'objects'),
  GIT_CONFIG_GLOBAL: join(required('HOME'), '.gitconfig'),
  GIT_TERMINAL_PROMPT: 'false',
};
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
async function fileDigest(path) {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest('hex');
}
const report = { started_at: new Date().toISOString(), binary, binary_sha256: await fileDigest(binary), commands: [], cases: [], verdict: 'running' };
function retain() {
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
}
function command(program, args, cwd = root, env = environment) {
  const directory = join(output, 'commands', randomUUID());
  mkdirSync(directory, { recursive: true });
  const stdout = join(directory, 'stdout');
  const stderr = join(directory, 'stderr');
  const observed = { program, args, cwd, stdout, stderr, state: 'running' };
  report.commands.push(observed);
  retain();
  const out = openSync(stdout, 'w');
  const err = openSync(stderr, 'w');
  let result;
  try { result = spawnSync(program, args, { cwd, env, stdio: ['ignore', out, err] }); }
  finally { closeSync(out); closeSync(err); }
  Object.assign(observed, { state: 'finished', status: result.status, signal: result.signal, error: result.error?.message });
  retain();
  return { ...result, stdout: readFileSync(stdout, 'utf8'), stderr: readFileSync(stderr, 'utf8') };
}
function success(program, args, cwd, env) {
  const result = command(program, args, cwd, env);
  assert.equal(result.status, successExit, `${program} ${args.join(' ')}: ${result.stderr}`);
  return result.stdout;
}
const pin = ['release', 'catalog', 'pin-input', checkout, '--name', 'swiftpm-cache', '--source', source, '--revision', revision, '--swiftpm', '--json'];
function refusal(name, args) {
  const before = readFileSync(manifestPath);
  const result = command(binary, args);
  assert.ok(result.status !== null && result.status !== successExit, `${name} was accepted`);
  assert.deepEqual(readFileSync(manifestPath), before);
  report.cases.push({ name, status: result.status, manifest_unchanged: true });
}
try {
  report.source_revision = success('git', ['rev-parse', 'HEAD']).trim();
  report.source_diff = success('git', ['diff', 'HEAD', '--', 'stado-rs/src/cli/release_catalog/publisher/input.rs', 'stado-rs/src/cli/release_catalog/publisher/input', 'stado-rs/product/src', 'tests/release/swiftpm-input.mjs']);
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.implementation_sha256 = {};
  for (const path of ['stado-rs/src/cli/release_catalog/publisher/input/swiftpm.rs', 'stado-rs/product/src/swift_cache.rs']) {
    report.implementation_sha256[path] = await fileDigest(join(root, path));
  }
  report.binary_version = success(binary, ['--version']).trim();
  report.swift_version = success('swift', ['--version']).trim();
  report.input_revision = success('git', ['rev-parse', '--verify', `${revision}^{commit}`], source).trim();
  const manifests = ['Package.swift', 'Package.resolved'].map(name => ({ name, bytes: readFileSync(join(source, name)) }));
  const receipt = JSON.parse(success(binary, pin));
  const saved = JSON.parse(readFileSync(manifestPath, 'utf8'));
  assert.equal(receipt.source_commit, report.input_revision);
  assert.deepEqual(saved.inputs['swiftpm-cache'], { uri: receipt.uri, sha256: receipt.sha256, mount: 'swiftpm-cache.tar.gz', extract: false });
  const archive = join(fixture, 'swiftpm-cache.tar.gz');
  success(binary, ['storage', 'get', receipt.uri, archive]);
  assert.equal(await fileDigest(archive), receipt.sha256);
  writeFileSync(join(restored, 'Package.resolved'), readFileSync(join(source, 'Package.resolved')));
  const restore = ['product', 'swift', '--package-path', restored, '--archive', archive, '--json', 'restore'];
  const restoredReceipt = JSON.parse(success(binary, restore));
  assert.equal(restoredReceipt.archive_sha256, receipt.sha256);
  assert.equal(restoredReceipt.reused, false);
  const scratch = join(restored, '.build');
  const consumerEnvironment = { ...environment, GIT_ALLOW_PROTOCOL: 'file' };
  const graph = JSON.parse(success('swift', ['package', '--package-path', source, '--scratch-path', scratch, '--disable-automatic-resolution', '--skip-update', 'show-dependencies', '--format', 'json'], root, consumerEnvironment));
  const state = JSON.parse(readFileSync(join(scratch, 'workspace-state.json'), 'utf8')).object;
  const lock = JSON.parse(readFileSync(join(source, 'Package.resolved'), 'utf8'));
  const observedPins = [];
  for (const pin of lock.pins) {
    const dependency = state.dependencies.find(value => value.packageRef.identity === pin.identity);
    assert.ok(dependency, `Restored resolution lacks ${pin.identity}`);
    const path = join(scratch, 'checkouts', dependency.subpath);
    assert.equal(success('git', ['rev-parse', 'HEAD'], path).trim(), pin.state.revision);
    observedPins.push({ identity: pin.identity, revision: pin.state.revision });
  }
  assert.deepEqual(new Set(state.dependencies.map(value => value.packageRef.identity)), new Set(lock.pins.map(value => value.identity)));
  for (const artifact of state.artifacts) {
    const path = realpathSync(artifact.path);
    const inside = relative(realpathSync(scratch), path);
    assert.ok(inside && !isAbsolute(inside) && inside !== '..' && !inside.startsWith('../'), `Artifact was not relocated: ${artifact.path}`);
    assert.ok(existsSync(path), `Artifact is absent: ${path}`);
  }
  assert.ok(state.artifacts.some(artifact => artifact.kind?.xcframework),
    'The real package fixture must include an XCFramework to exercise binary artifact relocation');
  for (const manifest of manifests) assert.deepEqual(readFileSync(join(source, manifest.name)), manifest.bytes);
  report.cases.push({ name: 'published-cache-consumed-at-another-path', receipt, graph, observed_pins: observedPins, artifacts: state.artifacts });
  refusal('swiftpm-path-conflict', [...pin, '--path', 'Package.swift']);
  refusal('swiftpm-cargo-conflict', [...pin, '--cargo']);
  const repeated = JSON.parse(success(binary, restore));
  assert.equal(repeated.reused, true);
  const artifact = state.artifacts.find(value => value.kind?.xcframework);
  const retainedArtifact = `${artifact.path}-${randomUUID()}`;
  renameSync(artifact.path, retainedArtifact);
  try {
    const missing = command(binary, restore);
    assert.ok(missing.status !== null && missing.status !== successExit,
      'Restore accepted a cache whose binary artifact disappeared');
    report.cases.push({ name: 'missing-restored-artifact-refused', status: missing.status });
  } finally {
    renameSync(retainedArtifact, artifact.path);
  }
  const dependency = state.dependencies.find(value => value.state.checkoutState);
  const checkoutPath = join(scratch, 'checkouts', dependency.subpath);
  const retainedCheckout = `${checkoutPath}-${randomUUID()}`;
  renameSync(checkoutPath, retainedCheckout);
  try {
    const missing = command(binary, restore);
    assert.ok(missing.status !== null && missing.status !== successExit,
      'Restore accepted a cache whose pinned checkout disappeared');
    report.cases.push({ name: 'missing-restored-checkout-refused', status: missing.status });
  } finally {
    renameSync(retainedCheckout, checkoutPath);
  }
  const lockPath = join(restored, 'Package.resolved');
  const originalLock = readFileSync(lockPath);
  const changedLock = JSON.parse(originalLock);
  changedLock.pins.find(value => value.identity === dependency.packageRef.identity).state.revision =
    success('git', ['rev-parse', '--verify', 'HEAD^'], checkoutPath).trim();
  writeFileSync(lockPath, JSON.stringify(changedLock));
  try {
    const stale = command(binary, restore);
    assert.ok(stale.status !== null && stale.status !== successExit,
      'Restore accepted a cache whose revision differs from Package.resolved');
    report.cases.push({ name: 'changed-pin-refused', status: stale.status });
  } finally {
    writeFileSync(lockPath, originalLock);
  }
  refusal('swiftpm-bundle-conflict', [...pin, '--git-bundle']);
  refusal('unknown-revision', pin.map(value => value === revision ? `refs/heads/example-absent-${randomUUID()}` : value));
  refusal('missing-source', pin.map(value => value === source ? join(fixture, 'absent-source') : value));
  report.verdict = 'passed';
} catch (error) {
  report.verdict = 'failed';
  report.error = String(error);
} finally {
  report.finished_at = new Date().toISOString();
  rmSync(fixture, { recursive: true, force: true });
  retain();
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') throw new Error(report.error);
