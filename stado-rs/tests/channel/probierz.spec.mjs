import { test, expect } from '@playwright/test';
import { execFile } from 'node:child_process';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const repo = resolve(process.env.PROBIERZ_APP_REPO || process.cwd());
const crate = resolve(repo, 'stado-rs');

// Immutable release 0.7.30 is intentionally retained by the public channel.
// Pinning it makes disappearance a contract failure rather than silently
// following whichever release happened to be newest when the journey ran.
const channel = 'https://lukaszs-macbook-pro-4007-2.tail6443b3.ts.net';
const version = '0.7.30';
const platform = 'darwin-arm64';

test('public release channel serves verified executable Stado bytes', async () => {
  const result = await exec('cargo', [
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
  });

  expect(result.stdout).toContain(
    'test public_release_channel_serves_a_verified_executable_native_release ... ok',
  );
  expect(result.stdout).toContain(`stado://${'releases'}/stado/${version}/${platform}`);
  expect(result.stdout).toContain('test result: ok. 1 passed; 0 failed');
});
