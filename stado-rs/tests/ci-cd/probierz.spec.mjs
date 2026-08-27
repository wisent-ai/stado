import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const skarbiec = process.env.SKARBIEC_TEST_BIN || join(homedir(), '.stado/bin/skarbiec');
let stdout;
let stderr;
try {
  ({ stdout, stderr } = await exec('cargo', [
    'test', '--test', 'ci-cd',
    'a_real_release_builds_publishes_and_installs_its_binary',
    '--', '--ignored', '--nocapture', '--test-threads=1',
  ], {
    cwd: crate,
    env: { ...process.env, SKARBIEC_TEST_BIN: skarbiec },
    timeout: 30 * 60 * 1000,
    maxBuffer: 4 * 1024 * 1024,
  }));
} catch (error) {
  const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
  process.stderr.write(`${output}\n`);
  throw new Error(`release journey failed with exit code ${error.code ?? 'unknown'}`);
}
assert.equal(stderr.includes('FAILED'), false, stderr);
assert.match(stdout, /a_real_release_builds_publishes_and_installs_its_binary \.\.\. ok/);
assert.ok(stdout.includes('test result: ok. 1 passed; 0 failed'));
process.stdout.write(stdout);
