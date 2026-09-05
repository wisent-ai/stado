import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

async function runReleaseTest(name, evidence) {
  let stdout;
  let stderr;
  try {
    ({ stdout, stderr } = await exec('cargo', [
      'test', '--locked', '--test', 'ci-cd', name,
      '--', '--ignored', '--exact', '--nocapture', '--test-threads=1',
    ], {
      cwd: crate,
      env: process.env,
      timeout: 30 * 60 * 1000,
      maxBuffer: 16 * 1024 * 1024,
    }));
  } catch (error) {
    const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
    process.stderr.write(`${output}\n`);
    throw new Error(`${name} failed with exit code ${error.code ?? 'unknown'}`);
  }
  assert.equal(stderr.includes('FAILED'), false, stderr);
  assert.match(stdout, new RegExp(`${name} \\.\\.\\.`));
  assert.match(stdout, evidence);
  assert.ok(stdout.includes('test result: ok. 1 passed; 0 failed'));
  process.stdout.write(stdout);
}

await runReleaseTest(
  'a_real_release_builds_publishes_and_installs_its_binary',
  /verified release platform=(darwin-arm64|linux-amd64); installed=ci-release-probe 1\.0\.0/,
);
await runReleaseTest(
  'a_cancelled_release_build_is_retried_under_a_new_job',
  /verified cancelled release retry platform=(darwin-arm64|linux-amd64); first_job=job-[a-f0-9]+; retry_job=job-[a-f0-9]+; installed=ci-release-probe 1\.0\.0/,
);
