import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const repo = resolve(process.env.PROBIERZ_APP_REPO || process.cwd());
const crate = resolve(repo, 'stado-rs');

// Pin the current immutable native release. Its disappearance is a contract
// failure; the test never follows a moving "latest" alias.
const channel = 'https://stado.wisent.com';
const version = '0.7.34';
const platform = 'darwin-arm64';

let stdout;
let stderr;
try {
  ({ stdout, stderr } = await exec('cargo', [
    'test', '--test', 'channel',
    'public_release_channel_serves_a_verified_executable_native_release',
    '--', '--ignored', '--nocapture',
  ], {
    cwd: crate,
    env: {
      ...process.env,
      STADO_RELEASE_CHANNEL_URL: channel,
      STADO_RELEASE_CHANNEL_VERSION: version,
      STADO_RELEASE_CHANNEL_PLATFORM: platform,
    },
    timeout: 20 * 60 * 1000,
    maxBuffer: 4 * 1024 * 1024,
  }));
} catch (error) {
  const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
  throw new Error(output.slice(-3000));
}

assert.equal(stderr.includes('FAILED'), false, stderr);
assert.match(stdout, /test public_release_channel_serves_a_verified_executable_native_release \.\.\. ok/);
assert.ok(stdout.includes(`stado://releases/stado/${version}/${platform}`));
assert.ok(stdout.includes('test result: ok. 1 passed; 0 failed'));
process.stdout.write(stdout);
