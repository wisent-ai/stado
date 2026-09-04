import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { constants as fsConstants } from 'node:fs';
import { access, chmod, mkdir, rm, writeFile } from 'node:fs/promises';
import { arch, homedir, platform } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

async function executable(path) {
  try {
    await access(path, fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function resolveSkarbiec() {
  const configured = process.env.SKARBIEC_TEST_BIN;
  const installed = configured || join(homedir(), '.stado/bin/skarbiec');
  if (await executable(installed)) {
    return installed;
  }
  if (configured) {
    throw new Error(`SKARBIEC_TEST_BIN is not executable: ${configured}`);
  }

  const key = `${platform()}-${arch()}`;
  const releases = {
    'darwin-arm64': {
      asset: 'skarbiec-v0.2.37-darwin-arm64.tar.gz',
      sha256: 'd113acc0d831bbefdce0308dbd311e5a6d14c8f9581c962abf380b3c2343743b',
    },
    'linux-x64': {
      asset: 'skarbiec-v0.2.37-linux-amd64.tar.gz',
      sha256: '45dc3869f869c347038cc97f3d454bf40f889219152c92862652c0c9e1166c89',
    },
  };
  const release = releases[key];
  assert.ok(release, `no real Skarbiec release is pinned for ${key}`);

  const cache = join(homedir(), '.cache/probierz/skarbiec/v0.2.37', key);
  const binary = join(cache, 'skarbiec');
  if (await executable(binary)) {
    return binary;
  }

  await mkdir(cache, { recursive: true, mode: 0o700 });
  const archive = join(cache, release.asset);
  const url = `https://github.com/wisent-ai/skarbiec/releases/download/v0.2.37/${release.asset}`;
  const response = await fetch(url);
  assert.equal(response.ok, true, `downloading real Skarbiec failed: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  assert.equal(
    createHash('sha256').update(bytes).digest('hex'),
    release.sha256,
    'downloaded real Skarbiec archive has the wrong digest',
  );
  await writeFile(archive, bytes, { mode: 0o600 });
  try {
    await exec('tar', ['-xzf', archive, '-C', cache]);
  } finally {
    await rm(archive, { force: true });
  }
  await chmod(binary, 0o700);
  assert.equal(await executable(binary), true, 'real Skarbiec archive contains no executable');
  return binary;
}

async function runReleaseTest(skarbiec, name, evidence) {
  let stdout;
  let stderr;
  try {
    ({ stdout, stderr } = await exec('cargo', [
      'test', '--locked', '--test', 'ci-cd', name,
      '--', '--ignored', '--exact', '--nocapture', '--test-threads=1',
    ], {
      cwd: crate,
      env: { ...process.env, SKARBIEC_TEST_BIN: skarbiec },
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

const skarbiec = await resolveSkarbiec();
await runReleaseTest(
  skarbiec,
  'a_real_release_builds_publishes_and_installs_its_binary',
  /verified release platform=(darwin-arm64|linux-amd64); installed=ci-release-probe 1\.0\.0/,
);
await runReleaseTest(
  skarbiec,
  'a_cancelled_release_build_is_retried_under_a_new_job',
  /verified cancelled release retry platform=(darwin-arm64|linux-amd64); first_job=job-[a-f0-9]+; retry_job=job-[a-f0-9]+; installed=ci-release-probe 1\.0\.0/,
);
