import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const tests = ['convergence_reloads_a_cached_private_stado_definition_once'];
const args = [
  'test', '--locked', '--test', 'native_readers',
  '--', '--ignored', '--nocapture', '--test-threads=1',
];

function run(file, commandArgs, options) {
  return new Promise((complete) => {
    execFile(file, commandArgs, options, (error, stdout, stderr) => {
      complete({ error, stdout: stdout || '', stderr: stderr || '' });
    });
  });
}

function sha256(text) {
  return createHash('sha256').update(text).digest('hex');
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

assert.equal(
  process.platform,
  'darwin',
  'native-readers requires the dedicated macOS host selected by Stado',
);
const artifacts = process.env.PROBIERZ_ARTIFACTS;
const mediaManifest = process.env.PROBIERZ_MEDIA_MANIFEST;
assert.ok(artifacts, 'PROBIERZ_ARTIFACTS is required');
assert.ok(mediaManifest, 'PROBIERZ_MEDIA_MANIFEST is required');

const result = await run('cargo', args, {
  cwd: crate,
  env: {
    ...process.env,
    CARGO_PROFILE_TEST_DEBUG: '0',
    CARGO_INCREMENTAL: '0',
  },
  encoding: 'utf8',
  timeout: 15 * 60 * 1000,
  maxBuffer: 8 * 1024 * 1024,
});
const exitCode = result.error
  ? (Number.isInteger(result.error.code) ? result.error.code : null)
  : 0;
const signal = result.error?.signal || null;
const stdoutPath = join(artifacts, 'stado-native-readers.stdout.log');
const stderrPath = join(artifacts, 'stado-native-readers.stderr.log');
const tracePath = join(artifacts, 'stado-native-readers.trace.json');
await mkdir(artifacts, { recursive: true });
await Promise.all([
  writeFile(stdoutPath, result.stdout, { mode: 0o600 }),
  writeFile(stderrPath, result.stderr, { mode: 0o600 }),
]);

const [revisionResult, statusResult] = await Promise.all([
  run('git', ['rev-parse', 'HEAD'], { cwd: repository, encoding: 'utf8' }),
  run('git', ['status', '--porcelain'], { cwd: repository, encoding: 'utf8' }),
]);
assert.equal(revisionResult.error, null, revisionResult.stderr);
assert.equal(statusResult.error, null, statusResult.stderr);
await writeFile(tracePath, `${JSON.stringify({
  schemaVersion: 1,
  kind: 'probierz-stado-cli-trace',
  journey: 'native-readers',
  runId: process.env.PROBIERZ_RUN_ID || null,
  status: exitCode === 0 ? 'completed' : 'failed',
  source: {
    repository,
    revision: revisionResult.stdout.trim(),
    dirty: statusResult.stdout.trim().length > 0,
  },
  process: {
    executable: 'cargo',
    args,
    cwd: crate,
    exitCode,
    signal,
    killed: Boolean(result.error?.killed),
    stdout: {
      file: stdoutPath,
      bytes: Buffer.byteLength(result.stdout),
      sha256: sha256(result.stdout),
    },
    stderr: {
      file: stderrPath,
      bytes: Buffer.byteLength(result.stderr),
      sha256: sha256(result.stderr),
    },
  },
  tests,
  productionMutations: 'one collision-resistant Probierz LaunchAgent in the selected macOS login domain; isolated HOME, storage, registry, port, logs, and binaries; removed through Stado service bootout and guarded host remove-file lifecycle commands',
  contracts: [
    'a real launchd unit can keep executing a private Stado file after its on-disk plist changes to the delivered root',
    'release converge-local-readers reloads that changed definition through the exact launchd domain observed to own it',
    'the public service label-print readback proves the replacement device, inode, executable path, and SHA-256 equal the delivered root file before convergence succeeds',
    'repeating convergence leaves an already-correct process running under the same pid',
  ],
  redaction: {
    status: 'verified_redacted',
    credentialsIncluded: false,
    productionIdentifiersIncluded: false,
  },
}, null, 2)}\n`, { mode: 0o600 });
await mkdir(dirname(mediaManifest), { recursive: true });
await writeFile(
  mediaManifest,
  `${JSON.stringify([{ file: tracePath, kind: 'trace', contentType: 'application/json' }], null, 2)}\n`,
  { mode: 0o600 },
);

process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
assert.equal(
  result.error,
  null,
  `native-readers journey failed with exit ${exitCode ?? 'unknown'}${signal ? ` (${signal})` : ''}`,
);
for (const test of tests) {
  assert.match(result.stdout, new RegExp(`test ${escapeRegExp(test)} \\.\\.\\. ok`));
}
