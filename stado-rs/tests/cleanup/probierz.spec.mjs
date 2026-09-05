import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const tests = [
  'dry_run_reports_eligible_cache_without_deleting_or_persisting',
  'enforce_deletes_only_tagged_cache_and_persists_reclaimed_progress',
  'overdue_lock_stays_report_only_until_the_predecessor_kernel_lock_is_released',
  'busy_lock_preserves_the_reclaim_hysteresis_and_scan_cursor',
  'once_and_watch_are_refused_with_the_public_usage_sentence',
];
const args = [
  'test', '--locked', '--test', 'cleanup',
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

const artifacts = process.env.PROBIERZ_ARTIFACTS;
const mediaManifest = process.env.PROBIERZ_MEDIA_MANIFEST;
assert.ok(artifacts, 'PROBIERZ_ARTIFACTS is required');
assert.ok(mediaManifest, 'PROBIERZ_MEDIA_MANIFEST is required');

const result = await run('cargo', args, {
  cwd: crate,
  env: process.env,
  encoding: 'utf8',
  timeout: 10 * 60 * 1000,
  maxBuffer: 8 * 1024 * 1024,
});
const exitCode = result.error
  ? (Number.isInteger(result.error.code) ? result.error.code : null)
  : 0;
const signal = result.error?.signal || null;
const stdoutPath = join(artifacts, 'stado-disk-cleanup.stdout.log');
const stderrPath = join(artifacts, 'stado-disk-cleanup.stderr.log');
const tracePath = join(artifacts, 'stado-disk-cleanup.trace.json');
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
  journey: 'disk-cleanup',
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
    stdout: { file: stdoutPath, bytes: Buffer.byteLength(result.stdout), sha256: sha256(result.stdout) },
    stderr: { file: stderrPath, bytes: Buffer.byteLength(result.stderr), sha256: sha256(result.stderr) },
  },
  tests,
  productionMutations: 'none: every story creates a separate temporary home, cache root, and local Stado store',
  contracts: [
    'preview reports an eligible tagged cache without deleting it or persisting janitor state',
    'enforcement deletes only the tagged cache and persists reclaimed progress',
    'an overdue predecessor lock remains report-only until its kernel lock is released',
    'a busy lock preserves reclaim hysteresis and the build-cache scan cursor',
    'the CLI refuses simultaneous --once and --watch with exit code 2 and its public usage sentence',
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
assert.equal(result.error, null, `disk-cleanup journey failed with exit ${exitCode ?? 'unknown'}${signal ? ` (${signal})` : ''}`);
assert.equal(result.stderr.includes('FAILED'), false, result.stderr);
for (const test of tests) {
  assert.match(result.stdout, new RegExp(`test ${escapeRegExp(test)} \\.\\.\\. ok`));
}
assert.ok(result.stdout.includes('test result: ok. 5 passed; 0 failed; 0 ignored'));
