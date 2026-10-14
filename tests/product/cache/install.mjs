import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync, rmSync, readdirSync, statSync, lstatSync, renameSync, symlinkSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Installs an actual catalogued Cargo CLI twice from the same canonical
// commit. The isolated home owns every installed file. Products with
// after-install actions or CLI dependencies are refused: this journey must
// not restart services or install another product into the operator's home.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
};
const binary = realpathSync(required('STADO_BIN'));
const source = realpathSync(required('STADO_PRODUCT_TEST_SOURCE'));
const product = required('STADO_PRODUCT_TEST_PRODUCT');
const cargoHome = required('CARGO_HOME');
const rustupHome = required('RUSTUP_HOME');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-cache-'));
const home = join(output, 'home');
mkdirSync(home);
const environment = { ...process.env, HOME: home, CARGO_HOME: cargoHome, RUSTUP_HOME: rustupHome,
  WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(home, 'store'),
  STADO_CONFIG: join(home, 'absent-config'), WISENT_OUTPUT_DIR: join(output, 'commands') };
const report = { result: 'failed', commands: [], product, source, binary, started_at: new Date().toISOString() };
const digest = value => createHash('sha256').update(value).digest('hex');

async function run(program, args, cwd = root) {
  const command = { program, arguments: args, cwd, stdout: '', stderr: '' };
  report.commands.push(command);
  const child = spawn(program, args, { cwd, env: environment, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.on('data', bytes => { command.stdout += bytes; process.stdout.write(bytes); });
  child.stderr.on('data', bytes => { command.stderr += bytes; process.stderr.write(bytes); });
  await new Promise((accept, reject) => {
    child.on('error', reject);
    child.on('close', (status, signal) => {
      command.exit_status = status;
      command.signal = signal;
      accept();
    });
  });
  return command;
}
function success(command) {
  assert.ok(!command.exit_status && !command.signal, command.stderr);
  return command.stdout.trim();
}
function evidence(state, name) {
  assert.equal(state.status, 'installed');
  assert.equal(state.source_revision, report.product_revision);
  assert.ok(state.cargo_cache, 'The source install must report its actual Cargo cache');
  const steps = JSON.parse(readFileSync(join(state.prepared.scratch, 'build-steps.json'), 'utf8'));
  assert.ok(steps.some(step => step.argv), 'The actual build must have retained its compiler evidence');
  writeFileSync(join(output, `${name}-build-steps.json`), JSON.stringify(steps, undefined, '  '));
  const receipt = JSON.parse(readFileSync(join(home, '.stado/products', product, 'cli.json'), 'utf8'));
  assert.equal(receipt.source_revision, report.product_revision);
  assert.deepEqual(receipt.cargo_cache, state.cargo_cache);
  const fingerprints = {};
  for (const path of receipt.installed_paths) {
    assert.ok(path.startsWith(`${home}/`), `Install escaped its isolated home: ${path}`);
    fingerprints[path] = digest(readFileSync(path));
    const recorded = receipt.placement_fingerprints[path];
    if (recorded.sha256) assert.equal(fingerprints[path], recorded.sha256);
  }
  const dependencies = join(state.cargo_cache.target, 'release/deps');
  const artifacts = Object.fromEntries(readdirSync(dependencies)
    .filter(name => name.endsWith('.rlib') || name.endsWith('.rmeta'))
    .map(name => {
      const path = join(dependencies, name);
      return [name, { sha256: digest(readFileSync(path)), modified: statSync(path).mtimeMs }];
    }));
  assert.ok(Object.keys(artifacts).some(name => name.endsWith('.rlib')),
    'The real Cargo build must produce dependency artifacts for the reuse check');
  return { steps, fingerprints, receipt, artifacts };
}
let failure;
try {
  report.source_revision = success(await run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(await run('git', ['diff', '--binary', 'HEAD']));
  report.binary_sha256 = digest(readFileSync(binary));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.product_revision = process.env.STADO_PRODUCT_TEST_COMMIT
    || success(await run('git', ['rev-parse', 'origin/main'], source));
  const base = ['product', '--workspace', dirname(source)];
  const catalog = JSON.parse(success(await run(binary, [...base, 'catalog', '--json'])));
  const entry = catalog.products.find(row => row.id === product);
  assert.ok(entry, `${product} is not catalogued`);
  const recipe = entry.installations.find(row => row.surface === 'cli');
  assert.equal(recipe.kind, 'stado-release', 'This journey exercises the source release installer');
  assert.ok(!recipe.after_install?.length, 'Use a product with no after-install actions');
  assert.ok(!entry.dependencies?.some(row => !row.for_surface || row.for_surface === 'cli'),
    'Use a standalone CLI so this journey cannot install or restart other products');
  const install = [...base, 'install', product, '--surface', 'cli', '--source-commit', report.product_revision, '--json'];
  const first = JSON.parse(success(await run(binary, install)));
  const before = evidence(first, 'first');
  report.first = first;
  const installedBinary = join(home, '.stado/bin', product);
  success(await run(installedBinary, ['--help']));
  const second = JSON.parse(success(await run(binary, install)));
  const after = evidence(second, 'second');
  report.second = second;
  assert.equal(second.cargo_cache.reused, true);
  assert.equal(second.cargo_cache.target, first.cargo_cache.target);
  assert.equal(second.cargo_cache.lockfile_sha256, first.cargo_cache.lockfile_sha256);
  assert.deepEqual(after.artifacts, before.artifacts,
    'A repeated installation must not rewrite compiled dependency artifacts');
  const compiled = after.steps.flatMap(step => [...`${step.stdout}\n${step.stderr}`.matchAll(/^\s*Compiling\s+(.+)$/gm)]
    .map(([, crate]) => crate));
  report.second_compiled_crates = compiled;
  assert.deepEqual(compiled, [], 'The second installation must compile no crate, including dependencies');
  success(await run(installedBinary, ['--help']));
  const [lockfile] = Object.keys(second.cargo_cache.lockfiles);
  assert.ok(lockfile, 'The real source cache must identify its committed lockfile');
  const cachedLock = join(second.cargo_cache.source, lockfile);
  const committedLock = readFileSync(cachedLock);
  const sentinel = join(home, 'cache-link-target');
  const sentinelBytes = Buffer.from('A cached input must not overwrite its symlink target.\n');
  writeFileSync(sentinel, sentinelBytes);
  rmSync(cachedLock);
  symlinkSync(sentinel, cachedLock);
  const restored = JSON.parse(success(await run(binary, install)));
  report.restored = restored;
  const restoration = evidence(restored, 'restored');
  assert.equal(lstatSync(cachedLock).isSymbolicLink(), false,
    'The next installation must restore the committed file type');
  assert.deepEqual(readFileSync(cachedLock), committedLock,
    'The next installation must restore the committed input');
  assert.deepEqual(readFileSync(sentinel), sentinelBytes,
    'Restoring the source cache must not follow a build-created symlink');
  success(await run(installedBinary, ['--help']));
  const cacheRoot = restored.cargo_cache.source;
  const retainedRoot = `${cacheRoot}-retained`;
  const beforeRootRefusal = readFileSync(join(home, '.stado/products', product, 'cli.json'));
  renameSync(cacheRoot, retainedRoot);
  symlinkSync(retainedRoot, cacheRoot);
  const refusedRoot = await run(binary, install);
  assert.ok(refusedRoot.exit_status && !refusedRoot.signal,
    'A symlink replacing the cache source root must be refused normally');
  assert.match(refusedRoot.stderr, /not a real directory/);
  assert.deepEqual(readFileSync(join(home, '.stado/products', product, 'cli.json')), beforeRootRefusal,
    'Refusing the cache source root must preserve the installed receipt');
  assert.deepEqual(readFileSync(join(retainedRoot, lockfile)), committedLock,
    'Refusing the cache source root must not mutate its target');
  rmSync(cacheRoot);
  renameSync(retainedRoot, cacheRoot);
  const receipt = join(home, '.stado/products', product, 'cli.json');
  const saved = readFileSync(receipt);
  const refused = await run(binary, [...base, 'install', product, '--surface', 'cli',
    '--source-commit', 'missing-install-cache-revision', '--json']);
  assert.ok(refused.exit_status && !refused.signal, 'An unknown revision must be refused normally');
  assert.deepEqual(readFileSync(receipt), saved, 'Refusing a revision must not change the installation');
  assert.deepEqual(Object.fromEntries(Object.keys(restoration.fingerprints).map(path => [path, digest(readFileSync(path))])),
    restoration.fingerprints, 'Refusing a revision must not replace installed bytes');
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = error.stack;
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, undefined, '  '));
  // Reports retain both compiler logs and receipts; all mutable product state
  // and compiled caches of this isolated journey are removed, even on failure.
  rmSync(home, { recursive: true, force: true });
  console.error(`report: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
