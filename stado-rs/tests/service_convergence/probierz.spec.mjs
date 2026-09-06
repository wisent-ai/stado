import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const test = 'authenticated_services_api_converges_real_same_host_state';
const args = [
  'test', '--locked', '--test', 'service_convergence', test,
  '--', '--ignored', '--exact', '--nocapture', '--test-threads=1',
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
  timeout: 20 * 60 * 1000,
  maxBuffer: 8 * 1024 * 1024,
});
const exitCode = result.error
  ? (Number.isInteger(result.error.code) ? result.error.code : null)
  : 0;
const signal = result.error?.signal || null;
const stdoutPath = join(artifacts, 'stado-service-convergence.stdout.log');
const stderrPath = join(artifacts, 'stado-service-convergence.stderr.log');
const tracePath = join(artifacts, 'stado-service-convergence.trace.json');
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
  journey: 'service-convergence',
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
  tests: [test],
  productionMutations: 'none: real Stado, storage, registry, grants, binaries and listener are isolated below the source target directory; the real Skarbiec download cache is checksum-pinned runner state',
  contracts: [
    'built Stado provisions the target-local verifier bearer through real Skarbiec without requiring or returning a JSON token',
    'repeated verifier provisioning preserves the exact owner-only bearer bytes and the resulting grant authorizes a real server-side item read',
    'symlink and empty existing bearer paths retain the source-grounded refusal and change neither the vault nor protected fixture state',
    'nonlocal GET and POST authenticate independently through action-scoped existing-registry-client grants stored in real Skarbiec',
    'malformed, unknown and unauthorized requests are refused without opening a host mutation',
    'host-wide and selected-binary reports use the real local hostname and Stado same-host execution rather than SSH',
    'a selected current source-built Stado binary succeeds without redundant delivery',
    'a missing declared Skarbiec release returns HTTP 200 with the complete nonzero convergence envelope and failed release diagnosis',
    'failed delivery preserves the installed source-built Stado, real Skarbiec and protected operator state byte-for-byte',
  ],
  redaction: {
    status: 'verified_redacted',
    credentialsIncluded: false,
    productionIdentifiersIncluded: false,
  },
}, null, 2)}\n`, { mode: 0o600 });
await writeFile(
  mediaManifest,
  `${JSON.stringify([{ file: tracePath, kind: 'trace', contentType: 'application/json' }], null, 2)}\n`,
  { mode: 0o600 },
);

if (result.error) {
  process.stderr.write(`${result.stdout}\n${result.stderr}\n`);
  throw new Error(`service-convergence journey failed with exit code ${exitCode ?? 'unknown'}`);
}
assert.equal(result.stderr.includes('FAILED'), false, result.stderr);
assert.match(result.stdout, new RegExp(`${test} \\.\\.\\.`));
assert.match(result.stdout, /verified authenticated nonlocal Services API on (darwin-arm64|linux-amd64)/);
assert.match(result.stdout, /verified persistent verifier bearer through built Stado and real Skarbiec/);
assert.ok(result.stdout.includes('token file must not be a symlink'));
assert.ok(result.stdout.includes('token file must be a nonempty regular file'));
assert.ok(result.stdout.includes('test result: ok. 1 passed; 0 failed'));
process.stdout.write(result.stdout);
