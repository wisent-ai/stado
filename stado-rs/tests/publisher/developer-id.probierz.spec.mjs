import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const repo = resolve(process.env.PROBIERZ_APP_REPO || process.cwd());
const crate = resolve(repo, 'stado-rs');
for (const name of [
  'STADO_PUBLISHER_TEST_TARGET',
  'STADO_PUBLISHER_TEST_REPOSITORY',
  'STADO_PUBLISHER_TEST_ACCOUNT_ITEM',
]) {
  assert.ok(process.env[name], `${name} must name the dedicated publisher fixture`);
}

let stdout;
let stderr;
try {
  ({ stdout, stderr } = await exec('cargo', [
    'test', '--test', 'publisher',
    'developer_id_issues_once_reuses_the_bundle_and_grants_repository_signing',
    '--', '--ignored', '--nocapture', '--test-threads=1',
  ], {
    cwd: crate,
    env: { ...process.env },
    timeout: 45 * 60 * 1000,
    maxBuffer: 4 * 1024 * 1024,
  }));
} catch (error) {
  const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
  throw new Error(output.slice(-6000));
}

assert.equal(stderr.includes('FAILED'), false, stderr);
assert.match(stdout, /developer_id_issues_once_reuses_the_bundle_and_grants_repository_signing \.\.\. ok/);
assert.ok(stdout.includes('test result: ok. 1 passed; 0 failed'));
process.stdout.write(stdout);
