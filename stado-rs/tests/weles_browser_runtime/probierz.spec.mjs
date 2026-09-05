import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
let stdout;
let stderr;
try {
  ({ stdout, stderr } = await exec('cargo', [
    'test', '--locked', '--test', 'weles_browser_runtime', '--', '--nocapture', '--test-threads=1',
  ], {
    cwd: crate,
    env: process.env,
    timeout: 10 * 60 * 1000,
    maxBuffer: 4 * 1024 * 1024,
  }));
} catch (error) {
  const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
  process.stderr.write(`${output}\n`);
  throw new Error(`weles-browser-runtime journey failed with exit code ${error.code ?? 'unknown'}`);
}
assert.equal(stderr.includes('FAILED'), false, stderr);
assert.ok(stdout.includes('test result: ok. 3 passed; 0 failed'), stdout);
process.stdout.write(stdout);

const artifacts = process.env.PROBIERZ_ARTIFACTS;
const mediaManifest = process.env.PROBIERZ_MEDIA_MANIFEST;
assert.ok(artifacts, 'PROBIERZ_ARTIFACTS is required');
assert.ok(mediaManifest, 'PROBIERZ_MEDIA_MANIFEST is required');
const [{ stdout: revision }, { stdout: status }] = await Promise.all([
  exec('git', ['rev-parse', 'HEAD'], { cwd: repository }),
  exec('git', ['status', '--porcelain'], { cwd: repository }),
]);
const tracePath = join(artifacts, 'stado-weles-browser-runtime.trace.json');
await mkdir(dirname(tracePath), { recursive: true });
await writeFile(tracePath, `${JSON.stringify({
  schemaVersion: 1,
  kind: 'probierz-stado-cli-trace',
  journey: 'weles-browser-runtime',
  runId: process.env.PROBIERZ_RUN_ID || null,
  status: 'completed',
  source: {
    repository,
    revision: revision.trim(),
    dirty: status.trim().length > 0,
  },
  productionMutations: 'none: the product binary used an isolated local Stado store and HOME',
  contracts: [
    'required component readiness is reported separately from browser-engine readiness',
    'a host with ffmpeg but no Chromium, Firefox, or WebKit is refused as browser_engine_missing',
    'a named missing browser engine reports the exact component repair command',
  ],
}, null, 2)}\n`, { mode: 0o600 });
await mkdir(dirname(mediaManifest), { recursive: true });
await writeFile(
  mediaManifest,
  `${JSON.stringify([{ file: tracePath, kind: 'trace', contentType: 'application/json' }], null, 2)}\n`,
  { mode: 0o600 },
);
