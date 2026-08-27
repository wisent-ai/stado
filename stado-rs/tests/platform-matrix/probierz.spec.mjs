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
  ['charless-mac-mini', 'darwin-arm64'],
  ['ubuntu-server-rtx-pro-6000', 'linux-amd64'],
];

const { stdout: revisionOutput } = await exec('git', ['rev-parse', 'HEAD'], { cwd: repository });
const revision = revisionOutput.trim();
assert.match(revision, /^[0-9a-f]{40}$/);

const verifyHost = async ([host, platform]) => {
  let stdout;
  let stderr;
  try {
    ({ stdout, stderr } = await exec(stado, [
      'host', 'verify-release-platform', host,
      '--repo', remote,
      '--ref', revision,
      '--json',
    ], {
      cwd: repository,
      timeout: 50 * 60 * 1000,
      maxBuffer: 16 * 1024 * 1024,
    }));
  } catch (error) {
    const output = `${error.stdout || ''}\n${error.stderr || ''}`.trim();
    process.stderr.write(`${output}\n`);
    throw new Error(`platform verification failed on ${host} with exit code ${error.code ?? 'unknown'}`);
  }

  assert.equal(stderr.includes('FAILED'), false, stderr);
  const evidence = JSON.parse(stdout);
  assert.equal(evidence.target, host);
  assert.equal(evidence.revision, revision);
  assert.equal(evidence.verified, true);
  assert.match(evidence.output, new RegExp(`verified recipe=probierz-native-build; job=.+; platform=${platform}; artifact=build-output.txt`));
  assert.match(evidence.output, new RegExp(`verified release platform=${platform}; installed=ci-release-probe 1.0.0`));
  assert.equal(evidence.output.includes('test result: FAILED'), false, evidence.output);
  return `verified host=${host}; platform=${platform}`;
};

const verified = [];
for (const host of hosts) {
  verified.push(await verifyHost(host));
}
process.stdout.write(`${verified.join('\n')}\n`);
