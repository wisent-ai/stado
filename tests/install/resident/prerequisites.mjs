// Dedicated-host journey: an older real provider, a refused consumer install,
// then qualified provider-first installation and persisted receipts.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, writeFileSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const program = process.env.STADO_BIN;
const revision = process.env.STADO_TEST_SOURCE_COMMIT;
const host = process.env.STADO_TEST_DEDICATED_HOST;
const operatorHome = process.env.HOME;
assert.ok(program && revision && host && operatorHome,
  'STADO_BIN, STADO_TEST_SOURCE_COMMIT, STADO_TEST_DEDICATED_HOST and HOME are required');
const output = join(root, '.build', 'install-prerequisites', createHash('sha256')
  .update(`${revision}:${host}:${process.pid}`).digest('hex'));
const home = join(output, 'home');
mkdirSync(home, { recursive: true });
const ciRecord = process.env.STADO_CI_MAIN_RECORD
  ? process.env.STADO_CI_MAIN_RECORD : join(operatorHome, '.stado', 'ci-main.json');
const temporary = join(home, 'tmp');
mkdirSync(temporary);
const toolEnvironment = Object.fromEntries(['PATH', 'CARGO_HOME', 'RUSTUP_HOME'].map(name => {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return [name, value];
}));
const environment = { ...toolEnvironment, HOME: home, TMPDIR: temporary,
  WISENT_WORKSPACE: dirname(root), STADO_CI_MAIN_RECORD: ciRecord,
  WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(home, 'store'),
  STADO_CONFIG: join(home, 'absent-config'),
  WISENT_OUTPUT_DIR: join(output, 'commands') };
const report = { result: 'failed', source_revision: revision, host, ci_record: ciRecord, commands: [] };
const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex');
function fingerprint(path) {
  const metadata = lstatSync(path);
  if (metadata.isSymbolicLink()) return { link: readlinkSync(path) };
  if (metadata.isDirectory()) {
    return Object.fromEntries(readdirSync(path).sort()
      .map(name => [name, fingerprint(join(path, name))]));
  }
  assert.ok(metadata.isFile(), `unsupported installed node: ${path}`);
  return { sha256: digest(path), mode: metadata.mode, modified: metadata.mtimeMs };
}
function execute(executable, arguments_, env = environment, cwd = root) {
  const result = spawnSync(executable, arguments_, { cwd, env, encoding: 'utf8' });
  report.commands.push({ executable, arguments: arguments_, exit_status: result.status,
    signal: result.signal, stdout: result.stdout, stderr: result.stderr,
    error: result.error?.message });
  return result;
}
function succeeds(result) {
  assert.ok(!result.error && !result.status && !result.signal,
    `command failed: ${result.error?.message} ${result.stderr} ${result.stdout}`);
  return result.stdout.trim();
}
const git = (checkout, arguments_) => succeeds(execute('git', ['-C', checkout, ...arguments_]));
const stado = arguments_ => execute(program, arguments_);
let failure;
try {
  report.binary_sha256 = digest(program);
  report.test_sha256 = digest(fileURLToPath(import.meta.url));
  assert.equal(git(root, ['rev-parse', 'HEAD']), revision, 'qualify the exact committed consumer');
  assert.equal(succeeds(execute(program, ['registry', 'self', '--local', '--name-only'],
    process.env)), host, 'the dedicated host must be this machine');
  const consumer = JSON.parse(git(root, ['show', `${revision}:.wisent-release.json`]));
  assert.equal(consumer.requires, undefined, 'the predecessor installer must accept the strict release manifest');
  const consumerVerbs = JSON.parse(git(root, ['show', `${revision}:contracts/verbs.json`]));
  assert.equal(consumerVerbs.product, consumer.product);
  const [required] = consumerVerbs.requires;
  assert.ok(required, 'consumer contract must declare another product and verb');
  const providerRoot = join(dirname(root), required.product);
  const [introduced] = git(providerRoot, ['log', '--reverse', '--format=%H',
    `-S${required.verb}`, 'HEAD', '--', 'contracts/verbs.json']).split('\n');
  assert.ok(introduced, 'provider contract history has no declared required verb');
  const [legacy] = git(providerRoot, ['log', '--reverse', '--format=%H',
    `-S${required.verb}`, 'HEAD', '--', '.wisent-release.json']).split('\n');
  assert.ok(legacy, 'the older provider must precede the strict-manifest declaration');
  const old = git(providerRoot, ['rev-parse', `${legacy}^`]);
  const oldManifest = JSON.parse(git(providerRoot, ['show', `${old}:.wisent-release.json`]));
  assert.ok(!oldManifest.provides?.some(item => item.verb === required.verb),
    'the older provider must not declare the verb');
  const ci = JSON.parse(readFileSync(ciRecord, 'utf8'));
  const green = ci.steps[`${required.product}:${required.surface}`]
    ?.findLast(item => item.green)?.revision;
  assert.ok(green, 'the real CI main record has no qualified provider revision');
  report.provider_old_revision = old;
  report.provider_qualified_revision = green;

  succeeds(stado(['product', 'install', required.product, '--surface', required.surface,
    '--source-commit', old, '--json']));
  const before = JSON.parse(succeeds(stado(['product', 'status', required.product,
    '--surface', required.surface, '--json'])));
  assert.equal(before.source_revision, old);
  assert.equal(before.status, 'installed');
  report.before = before;

  const planned = JSON.parse(succeeds(stado(['product', 'install', consumer.product,
    '--surface', 'cli', '--source-commit', revision, '--plan', '--json'])));
  const step = planned.chain.find(item => item.product === required.product && item.verb === required.verb);
  assert.ok(step, 'plan must name the real provider and required verb');
  assert.equal(step.action, 'install');
  assert.equal(step.minimum_revision, introduced);
  assert.equal(step.revision, green);
  assert.equal(step.installed_revision, old);
  const [target] = planned.chain.slice().reverse();
  assert.equal(target.product, consumer.product);
  report.plan = planned;
  const excluded = stado(['product', 'install', consumer.product, '--surface', 'cli',
    '--source-commit', revision, '--without', required.product, '--plan', '--json']);
  assert.ok(excluded.status || excluded.signal || excluded.error,
    'an explicit exclusion cannot suppress a required verb');
  assert.ok(excluded.stderr.includes(`--without ${required.product} cannot provide it`));
  report.exclusion = { exit_status: excluded.status, stderr: excluded.stderr };


  const refused = stado(['product', 'install', consumer.product, '--surface', 'cli',
    '--source-commit', revision, '--json']);
  assert.ok(refused.status || refused.signal || refused.error,
    'the consumer must refuse missing verb before installation');
  assert.ok(refused.stderr.includes(`${required.product} ${required.surface} needs verb ${required.verb} from revision`));
  assert.match(refused.stderr, /--with-prerequisites/);
  const stillOld = JSON.parse(succeeds(stado(['product', 'status', required.product,
    '--surface', required.surface, '--json'])));
  assert.equal(stillOld.source_revision, old);
  report.refusal = { exit_status: refused.status, stderr: refused.stderr };

  succeeds(stado(['product', 'install', consumer.product, '--surface', 'cli',
    '--source-commit', revision, '--with-prerequisites', '--json']));
  const provider = JSON.parse(succeeds(stado(['product', 'status', required.product,
    '--surface', required.surface, '--json'])));
  const installedConsumer = JSON.parse(succeeds(stado(['product', 'status', consumer.product,
    '--surface', 'cli', '--json'])));
  assert.equal(provider.status, 'installed');
  assert.equal(provider.source_revision, green);
  assert.ok(provider.installed_paths?.length, 'provider files must be recorded');
  assert.equal(installedConsumer.status, 'installed');
  assert.equal(installedConsumer.source_revision, revision);
  assert.ok(installedConsumer.installed_paths?.length, 'consumer files must be recorded');
  report.after = { provider, consumer: installedConsumer };
  const providerReceipt = join(home, '.stado', 'products', required.product,
    `${required.surface}.json`);
  const retainedReceipt = readFileSync(providerReceipt);
  const retainedFiles = Object.fromEntries(provider.installed_paths
    .map(path => [path, fingerprint(path)]));
  const reusePlan = JSON.parse(succeeds(stado(['product', 'install', consumer.product,
    '--surface', 'cli', '--source-commit', revision, '--plan', '--json'])));
  const reuseStep = reusePlan.chain.find(item =>
    item.product === required.product && item.verb === required.verb);
  assert.ok(reuseStep, 'repeat plan must retain the prerequisite decision');
  assert.equal(reuseStep.action, 'reuse');
  assert.equal(reuseStep.installed_revision, green);
  report.reuse_plan = reusePlan;
  // Omit --with-prerequisites: the installed provider must already satisfy the verb.
  const repeated = JSON.parse(succeeds(stado(['product', 'install', consumer.product,
    '--surface', 'cli', '--source-commit', revision, '--json'])));
  assert.equal(repeated.status, 'installed');
  assert.equal(repeated.source_revision, revision);
  assert.deepEqual(readFileSync(providerReceipt), retainedReceipt,
    'reinstalling the consumer must not replace the satisfied provider receipt');
  assert.deepEqual(Object.fromEntries(provider.installed_paths
    .map(path => [path, fingerprint(path)])), retainedFiles,
  'reinstalling the consumer must preserve provider bytes, modes and modification times');
  report.repeated = { consumer: repeated, provider: JSON.parse(retainedReceipt.toString()),
    files: retainedFiles };
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
