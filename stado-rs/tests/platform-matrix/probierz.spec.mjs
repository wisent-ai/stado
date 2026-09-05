import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { createHash } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const remote = 'https://github.com/wisent-ai/stado.git';
const stado = resolve(crate, 'target/release/stado');
const linuxWorker = 'local-ubuntu-server';
const artifacts = process.env.PROBIERZ_ARTIFACTS;
const probierzRunId = process.env.PROBIERZ_RUN_ID;
assert.ok(artifacts && probierzRunId, 'run the platform matrix through Probierz');

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
    process.stderr.write(`${output}\n`);
    throw new Error(`platform verification failed on charless-mac-mini with exit code ${error.code ?? 'unknown'}`);
  }

  assert.equal(stderr.includes('FAILED'), false, stderr);
  const evidence = JSON.parse(stdout);
  await writeFile(resolve(artifacts, 'release-platform-darwin.json'), stdout);
  assert.equal(evidence.target, 'charless-mac-mini');
  assert.equal(evidence.revision, revision);
  assert.equal(evidence.verified, true);
  assert.match(evidence.output, /verified recipe=probierz-native-build; job=.+; platform=darwin-arm64; artifact=build-output.txt/);
  assert.match(evidence.output, /verified release platform=darwin-arm64; installed=ci-release-probe 1.0.0/);
  assert.match(evidence.output, /verified cancelled release retry platform=darwin-arm64; first_job=job-[a-f0-9]+; retry_job=job-[a-f0-9]+; installed=ci-release-probe 1\.0\.0/);
  assert.equal(evidence.output.includes('test result: FAILED'), false, evidence.output);
  return 'verified host=charless-mac-mini; platform=darwin-arm64';
};

const linuxCommand = [
  'set -eu',
  'export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:$PATH"',
  'export CARGO_TARGET_DIR="$PWD/.wisent-output/platform-matrix-cargo-target"',
  'export TMPDIR="$PWD/.wisent-output/tmp"',
  'mkdir -p "$TMPDIR"',
  'export TMP="$TMPDIR" TEMP="$TMPDIR"',
  'curl -fsSLo .probierz-skarbiec.tar.gz https://github.com/wisent-ai/skarbiec/releases/download/v0.1.3/skarbiec-v0.1.3-linux-amd64.tar.gz',
  'printf "4433afe3372d2c35cb33420307f5efe8b6e3b01bd7907b18d1d9c2b471f9ee68  .probierz-skarbiec.tar.gz\\n" | sha256sum -c -',
  'mkdir .probierz-skarbiec-bin',
  'tar -xzf .probierz-skarbiec.tar.gz -C .probierz-skarbiec-bin',
  'export SKARBIEC_TEST_BIN="$PWD/.probierz-skarbiec-bin/skarbiec"',
  'test -x "$SKARBIEC_TEST_BIN"',
  'cd stado-rs',
  'cargo test --locked --test builds build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact -- --ignored --exact --nocapture --test-threads=1',
  'cargo test --locked --test ci-cd a_real_release_builds_publishes_and_installs_its_binary -- --ignored --exact --nocapture --test-threads=1',
  'cargo test --locked --test ci-cd a_cancelled_release_build_is_retried_under_a_new_job -- --ignored --exact --nocapture --test-threads=1',
].join(' && ');
const verifyLinux = async () => {
  const runId = `probierz-platform-${createHash('sha256').update(`${probierzRunId}:linux-amd64`).digest('hex').slice(0, 24)}`;
  let submitted;
  let watched;
  try {
    submitted = await exec(stado, [
      'submit', linuxCommand,
      '--run-id', runId,
      '--provider', 'local',
      '--pin-provider',
      '--pinned-host', linuxWorker,
      '--repo', remote,
      '--repo-ref', revision,
      '--repo-extras', '',
    ], { cwd: repository, timeout: 120_000, maxBuffer: 4 * 1024 * 1024 });
    const jobId = submitted.stdout.match(/Job ID: (job-[0-9a-f]{24})/)?.[1];
    assert.ok(jobId, `submit on ${linuxWorker} returned no job id: ${submitted.stdout}\n${submitted.stderr}`);
    await writeFile(resolve(artifacts, 'release-platform-linux-submission.json'), JSON.stringify({
      runId, jobId, revision, host: linuxWorker,
    }, null, 2));

    watched = await exec(stado, ['job', 'watch', jobId, '--follow', '--json'], {
      cwd: repository,
      timeout: 50 * 60 * 1000,
      maxBuffer: 16 * 1024 * 1024,
    });
    await writeFile(resolve(artifacts, 'release-platform-linux.json'), watched.stdout);
    const evidence = JSON.parse(watched.stdout);
    assert.equal(evidence.terminal, true);
    assert.equal(evidence.job.state, 'completed');
    assert.match(evidence.log, /verified recipe=probierz-native-build; job=.+; platform=linux-amd64; artifact=build-output.txt/);
    assert.match(evidence.log, /verified release platform=linux-amd64; installed=ci-release-probe 1\.0\.0/);
    assert.match(evidence.log, /verified cancelled release retry platform=linux-amd64; first_job=job-[a-f0-9]+; retry_job=job-[a-f0-9]+; installed=ci-release-probe 1\.0\.0/);
    return `verified host=${linuxWorker}; platform=linux-amd64; job=${jobId}`;
  } catch (error) {
    const output = `${error.stdout || watched?.stdout || submitted?.stdout || ''}\n${error.stderr || watched?.stderr || submitted?.stderr || ''}`.trim();
    await writeFile(resolve(artifacts, 'release-platform-linux-error.log'), `${output}\n`);
    process.stderr.write(`${output}\n`);
    throw error;
  }
};
const platform = process.env.PROBIERZ_PLATFORM;
const verified = [];
if (!platform || platform === 'darwin-arm64') verified.push(await verifyDarwin());
if (!platform || platform === 'linux-amd64') verified.push(await verifyLinux());
assert.ok(verified.length > 0, `unsupported PROBIERZ_PLATFORM=${platform}`);
process.stdout.write(`${verified.join('\n')}\n`);
