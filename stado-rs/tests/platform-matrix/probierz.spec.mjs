import { execFile } from 'node:child_process';
import { strict as assert } from 'node:assert';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const crate = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const repository = resolve(crate, '..');
const stado = process.env.TUI_CMD || resolve(crate, 'target/release/stado');
const remote = 'https://github.com/wisent-ai/stado.git';
const hosts = [
  ['local-ubuntu-server', 'linux-amd64'],
];

const { stdout: revisionOutput } = await exec('git', ['rev-parse', 'HEAD'], { cwd: repository });
const revision = revisionOutput.trim();
assert.match(revision, /^[0-9a-f]{40}$/);
await import('../builds/probierz.spec.mjs');
await import('../ci-cd/probierz.spec.mjs');
process.stdout.write('verified host=local; platform=darwin-arm64\n');

const command = [
  'set -eu',
  'git clone --depth 1 https://github.com/wisent-ai/skarbiec.git .probierz-skarbiec',
  'cargo build --release --manifest-path .probierz-skarbiec/Cargo.toml',
  'export SKARBIEC_TEST_BIN="$PWD/.probierz-skarbiec/target/release/skarbiec"',
  "cargo test --test builds build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact -- --ignored --nocapture --test-threads=1",
  "cargo test --test ci-cd a_real_release_builds_publishes_and_installs_its_binary -- --ignored --nocapture --test-threads=1",
].join('; ');

const verifyHost = async ([host, platform]) => {
  const submitted = await exec(stado, [
    'submit', command,
    '--provider', 'local',
    '--pin-provider',
    '--pinned-host', host,
    '--repo', remote,
    '--repo-ref', revision,
    '--repo-extras', '',
  ], { cwd: repository, timeout: 120_000, maxBuffer: 4 * 1024 * 1024 });
  const jobId = submitted.stdout.match(/Job ID: ([0-9a-f]{8})/i)?.[1];
  assert.ok(jobId, `submit on ${host} returned no job id: ${submitted.stdout}\n${submitted.stderr}`);

  const watched = await exec(stado, ['job', 'watch', jobId, '--follow', '--json'], {
    cwd: repository,
    timeout: 40 * 60 * 1000,
    maxBuffer: 16 * 1024 * 1024,
  });
  const evidence = `${watched.stdout}\n${watched.stderr}`;
  assert.match(evidence, new RegExp(`verified recipe=probierz-native-build; job=.+; platform=${platform}; artifact=build-output.txt`));
  assert.match(evidence, new RegExp(`verified release platform=${platform}; installed=ci-release-probe 1.0.0`));
  assert.equal(evidence.includes('test result: FAILED'), false, evidence);
  return `verified host=${host}; platform=${platform}; job=${jobId}`;
};

const verified = await Promise.all(hosts.map(verifyHost));
process.stdout.write(`${verified.join('\n')}\n`);
