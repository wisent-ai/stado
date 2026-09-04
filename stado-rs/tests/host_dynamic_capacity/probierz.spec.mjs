import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const test = 'host_gates_use_live_resources_and_never_fixed_slots';
let stdout;
let stderr;
try {
  ({ stdout, stderr } = await exec('cargo', [
    'test', '--locked', '--test', 'host_dynamic_capacity', test,
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
  throw new Error(`host-dynamic-capacity journey failed with exit code ${error.code ?? 'unknown'}`);
}
assert.equal(stderr.includes('FAILED'), false, stderr);
assert.match(stdout, new RegExp(`${test} \\.\\.`));
assert.ok(stdout.includes('test result: ok. 1 passed; 0 failed'));
process.stdout.write(stdout);

const artifacts = process.env.PROBIERZ_ARTIFACTS;
const mediaManifest = process.env.PROBIERZ_MEDIA_MANIFEST;
assert.ok(artifacts, 'PROBIERZ_ARTIFACTS is required');
assert.ok(mediaManifest, 'PROBIERZ_MEDIA_MANIFEST is required');
const [{ stdout: revision }, { stdout: status }] = await Promise.all([
  exec('git', ['rev-parse', 'HEAD'], { cwd: repository }),
  exec('git', ['status', '--porcelain'], { cwd: repository }),
]);
const tracePath = join(artifacts, 'stado-host-dynamic-capacity.trace.json');
await mkdir(dirname(tracePath), { recursive: true });
await writeFile(tracePath, `${JSON.stringify({
  schemaVersion: 1,
  kind: 'probierz-stado-cli-trace',
  journey: 'host-dynamic-capacity',
  runId: process.env.PROBIERZ_RUN_ID || null,
  status: 'completed',
  source: {
    repository,
    revision: revision.trim(),
    dirty: status.trim().length > 0,
  },
  test,
  productionMutations: 'none: the product binary used an isolated local Stado store',
  contracts: [
    'host gates reports live CPU, RAM, VRAM, accelerator, and running-job capacity',
    'the public JSON and human output contain no fixed slot count',
    'a paused host is refused with its exact blocker sentence',
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
