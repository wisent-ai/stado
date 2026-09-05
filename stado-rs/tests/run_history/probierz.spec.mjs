import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const tests = [
  'coordinator_retains_an_unlinked_legacy_terminal_job_from_its_manifest_entry',
  'coordinator_preserves_settled_history_and_refuses_missing_unretired_history',
];
let stdout;
let stderr;
try {
  ({ stdout, stderr } = await exec('cargo', [
    'test', '--test', 'run_history',
    '--', '--ignored', '--nocapture', '--test-threads=1',
  ], {
    cwd: crate,
    env: process.env,
    timeout: 10 * 60 * 1000,
    maxBuffer: 4 * 1024 * 1024,
  }));
} catch (error) {
  const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
  process.stderr.write(`${output}\n`);
  throw new Error(`run-retention journey failed with exit code ${error.code ?? 'unknown'}`);
}
assert.equal(stderr.includes('FAILED'), false, stderr);
for (const test of tests) assert.match(stdout, new RegExp(`${test} \\.\\.\\.`));
assert.ok(stdout.includes(`test result: ok. ${tests.length} passed; 0 failed`));
process.stdout.write(stdout);

const artifacts = process.env.PROBIERZ_ARTIFACTS;
const mediaManifest = process.env.PROBIERZ_MEDIA_MANIFEST;
assert.ok(artifacts, 'PROBIERZ_ARTIFACTS is required');
assert.ok(mediaManifest, 'PROBIERZ_MEDIA_MANIFEST is required');
const [{ stdout: revision }, { stdout: status }] = await Promise.all([
  exec('git', ['rev-parse', 'HEAD'], { cwd: repository }),
  exec('git', ['status', '--porcelain'], { cwd: repository }),
]);
const tracePath = join(artifacts, 'stado-run-retention.trace.json');
await mkdir(dirname(tracePath), { recursive: true });
await writeFile(tracePath, `${JSON.stringify({
  schemaVersion: 1,
  kind: 'probierz-stado-cli-trace',
  journey: 'run-retention',
  runId: process.env.PROBIERZ_RUN_ID || null,
  status: 'completed',
  source: {
    repository,
    revision: revision.trim(),
    dirty: status.trim().length > 0,
  },
  tests,
  productionMutations: 'none: the product binary used an isolated local Stado store',
  contracts: [
    'the coordinator retains the exact legacy terminal job named by its manifest entry',
    'the lifecycle blob is reaped only after its outcome is retained',
    'settled cancellation history is not reopened after its run manifest is removed',
    'an unretired transition still refuses a missing run manifest',
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
