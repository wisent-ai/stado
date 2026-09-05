import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

assert.equal(process.platform, 'darwin', 'Apple preparation requires the dedicated real Mac');
assert.equal(process.arch, 'arm64');
assert.ok(process.env.STADO_APPLE_PREPARATION_HOST?.trim(), 'STADO_APPLE_PREPARATION_HOST must explicitly name the registered Apple host');
const repo = resolve(process.env.PROBIERZ_APP_SOURCE || process.env.PROBIERZ_APP_REPO || process.cwd());
const exec = promisify(execFile);
const args = ['test', '--release', '--test', 'apple_challenge', '--', '--ignored', '--nocapture', '--test-threads=1'];
process.stdout.write(`Running cargo ${args.join(' ')} from ${repo}/stado-rs\n`);
let result;
try {
  result = await exec('cargo', args, {
    cwd: resolve(repo, 'stado-rs'),
    env: { ...process.env },
    timeout: 15 * 60 * 1000,
    maxBuffer: 16 * 1024 * 1024,
  });
} catch (error) {
  process.stdout.write(error.stdout || '');
  process.stderr.write(error.stderr || '');
  throw error;
}
process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
assert.match(result.stdout, /test result: ok\. 1 passed; 0 failed/, 'the real Apple preparation story must execute, not merely compile or be filtered out');
