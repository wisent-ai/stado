import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const remote = 'https://github.com/wisent-ai/stado.git';

const { stdout: revisionOutput } = await exec('git', ['rev-parse', 'HEAD'], { cwd: repository });
const revision = revisionOutput.trim();
assert.match(revision, /^[0-9a-f]{40}$/);

const verifyDarwin = async () => {
  let stdout;
  let stderr;
  try {
    ({ stdout, stderr } = await exec('cargo', [
      'run', '--release', '--bin', 'stado', '--',
      'host', 'verify-release-platform', 'charless-mac-mini',
      '--repo', remote,
      '--ref', revision,
      '--json',
    ], {
      cwd: crate,
      timeout: 50 * 60 * 1000,
      maxBuffer: 16 * 1024 * 1024,
    }));
  } catch (error) {
    const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
    const tail = output.split('\n').slice(-12).join('\n');
    throw new Error(`platform verification failed on charless-mac-mini with exit code ${error.code ?? 'unknown'}\n${tail}`);
  }

  assert.equal(stderr.includes('FAILED'), false, stderr);
  const evidence = JSON.parse(stdout);
  assert.equal(evidence.target, 'charless-mac-mini');
  assert.equal(evidence.revision, revision);
  assert.equal(evidence.verified, true);
  assert.match(evidence.output, /verified recipe=probierz-native-build; job=.+; platform=darwin-arm64; artifact=build-output.txt/);
  assert.match(evidence.output, /verified release platform=darwin-arm64; installed=ci-release-probe 1.0.0/);
  assert.equal(evidence.output.includes('test result: FAILED'), false, evidence.output);
  return 'verified host=charless-mac-mini; platform=darwin-arm64';
};

const linuxCommand = [
  'set -eu',
  'export PATH="/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin:$PATH"',
  'export CARGO_TARGET_DIR="/cargo-target"',
  'mkdir -p "$CARGO_TARGET_DIR"',
  'curl -fsSLo .probierz-skarbiec.tar.gz https://github.com/wisent-ai/skarbiec/releases/download/v0.1.3/skarbiec-v0.1.3-linux-amd64.tar.gz',
  'printf "4433afe3372d2c35cb33420307f5efe8b6e3b01bd7907b18d1d9c2b471f9ee68  .probierz-skarbiec.tar.gz\\n" | sha256sum -c -',
  'mkdir .probierz-skarbiec-bin',
  'tar -xzf .probierz-skarbiec.tar.gz -C .probierz-skarbiec-bin',
  'export SKARBIEC_TEST_BIN="$PWD/.probierz-skarbiec-bin/skarbiec"',
  'test -x "$SKARBIEC_TEST_BIN"',
  'cd stado-rs',
  'cargo test --test builds build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact -- --ignored --nocapture --test-threads=1',
  'cargo test --test ci-cd a_real_release_builds_publishes_and_installs_its_binary -- --ignored --nocapture --test-threads=1',
].join(' && ');
const verifyLinux = async () => {
  const checkout = [
    'git init -q /work',
    'cd /work',
    `git remote add origin ${remote}`,
    `git fetch -q --depth 1 origin ${revision}`,
    'git checkout -q --detach FETCH_HEAD',
    linuxCommand,
  ].join(' && ');
  const { stdout, stderr } = await exec('docker', [
    'run', '--rm',
    '--platform', 'linux/amd64',
    '-v', 'stado-platform-matrix-target:/cargo-target',
    'rust:1-bookworm',
    'bash', '-lc', checkout,
  ], {
    cwd: repository,
    timeout: 50 * 60 * 1000,
    maxBuffer: 16 * 1024 * 1024,
  });
  const evidence = `${stdout}\n${stderr}`;
  assert.match(evidence, /verified recipe=probierz-native-build; job=.+; platform=linux-amd64; artifact=build-output.txt/);
  assert.match(evidence, /verified release platform=linux-amd64; installed=ci-release-probe 1.0.0/);
  assert.equal(evidence.includes('test result: FAILED'), false, evidence);
  return 'verified runtime=docker-linux-amd64; platform=linux-amd64';
};
const platform = process.env.PROBIERZ_PLATFORM;
const verified = [];
if (!platform || platform === 'darwin-arm64') verified.push(await verifyDarwin());
if (!platform || platform === 'linux-amd64') verified.push(await verifyLinux());
assert.ok(verified.length > 0, `unsupported PROBIERZ_PLATFORM=${platform}`);
process.stdout.write(`${verified.join('\n')}\n`);
