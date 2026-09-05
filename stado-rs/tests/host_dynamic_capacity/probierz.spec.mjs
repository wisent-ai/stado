import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const tests = [
  'host_gates_use_live_resources_and_never_fixed_slots',
  'registry_policy_rewrite_removes_legacy_fixed_capacity_declarations',
  'live_resources_admit_two_jobs_despite_legacy_single_worker_limits',
];
const args = [
  'test', '--locked', '--test', 'host_dynamic_capacity', '--test', 'capacity',
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
  env: {
    ...process.env,
    CARGO_PROFILE_TEST_DEBUG: '0',
    CARGO_INCREMENTAL: '0',
  },
  encoding: 'utf8',
  timeout: 10 * 60 * 1000,
  maxBuffer: 8 * 1024 * 1024,
});
const exitCode = result.error
  ? (Number.isInteger(result.error.code) ? result.error.code : null)
  : 0;
const signal = result.error?.signal || null;
const stdoutPath = join(artifacts, 'stado-host-dynamic-capacity.stdout.log');
const stderrPath = join(artifacts, 'stado-host-dynamic-capacity.stderr.log');
const tracePath = join(artifacts, 'stado-host-dynamic-capacity.trace.json');
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
  journey: 'host-dynamic-capacity',
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
  productionMutations: 'none: every story uses an isolated local Stado store and the worker runs only its submitted workloads',
  contracts: [
    'host gates reports live CPU, RAM, VRAM, accelerator, and running-job capacity',
    'the public JSON and human output contain no fixed slot count',
    'a paused host is refused with its exact blocker sentence',
    'a registry policy write removes retired fixed worker-cap declarations',
    'a real worker runs both submitted workloads concurrently despite legacy caps of one and persists both completed job records',
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
assert.equal(result.error, null, `host-dynamic-capacity journey failed with exit ${exitCode ?? 'unknown'}${signal ? ` (${signal})` : ''}`);
for (const test of tests) {
  assert.match(result.stdout, new RegExp(`test ${escapeRegExp(test)} \\.\\.\\. ok`));
}
