import { test, expect } from '@playwright/test';
import { execFile } from 'node:child_process';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const repo = resolve(process.env.PROBIERZ_APP_REPO || process.cwd());
const crate = join(repo, 'stado-rs');
const skarbiec = process.env.SKARBIEC_TEST_BIN || join(homedir(), '.stado/bin/skarbiec');

test('Stado writes to and field-reads from the real Skarbiec binary', async () => {
  const result = await exec('cargo', [
    'test', '--test', 'secrets', '--', '--ignored', '--nocapture',
  ], {
    cwd: crate,
    env: { ...process.env, SKARBIEC_TEST_BIN: skarbiec },
    timeout: 15 * 60 * 1000,
    maxBuffer: 1024 * 1024,
  });
  expect(result.stdout).toContain('test secrets_put_writes_a_typed_item_to_real_skarbiec ... ok');
  expect(result.stdout).toContain('test secrets_get_reads_only_the_granted_field_from_real_skarbiec ... ok');
  expect(result.stdout).toContain('test result: ok. 2 passed; 0 failed');
});
