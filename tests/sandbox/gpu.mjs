import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';
import { exitedCleanly } from '../native/process.mjs';

// Actual GPU passthrough to a gVisor fleet sandbox through nvproxy, on a
// registered Linux host with an NVIDIA GPU. No simulated device or host.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const area = join(root, '.build', 'sandbox');
mkdirSync(area, { recursive: true });
const output = mkdtempSync(join(area, 'gpu-'));
const report = { result: 'blocked', started_at: new Date().toISOString(), commands: [] };
const name = `gpu-qualification-${randomUUID()}`;
let binary;
let host;
let created = false;
let failure;
function run(args) {
  const answer = spawnSync(binary, args, { cwd: root, env: process.env, encoding: 'utf8' });
  report.commands.push({ program: binary, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function receipt(answer) {
  assert.ok(exitedCleanly(answer), answer.stderr || answer.error?.message);
  const document = JSON.parse(answer.stdout);
  assert.equal(document.ok, true, JSON.stringify(document));
  return document;
}
const fleet = (verb, extra = []) => run(['sandbox', 'fleet', verb, name, '--host', host, ...extra]);
try {
  const required = ['STADO_BIN', 'STADO_TEST_SOURCE_REVISION', 'STADO_SANDBOX_GPU_HOST',
    'STADO_SANDBOX_GPU_IMAGE', 'STADO_SANDBOX_TEST_CPUS', 'STADO_SANDBOX_TEST_MEMORY_BYTES',
    'STADO_SANDBOX_TEST_PIDS_LIMIT', 'STADO_SANDBOX_GPU_COUNT', 'STADO_SANDBOX_GPU_COMMAND'];
  const missing = required.filter(key => !process.env[key]);
  assert.deepEqual(missing, [], `the GPU journey requires explicit inputs: ${missing.join(', ')}`);
  report.subject = snapshotSubject(process.env.STADO_BIN, output);
  binary = report.subject.path;
  host = process.env.STADO_SANDBOX_GPU_HOST;
  report.source_revision = process.env.STADO_TEST_SOURCE_REVISION;
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  verifyRevision(receiptless(run(['--version'])), report.source_revision);
  report.result = 'failed';
  const gpus = process.env.STADO_SANDBOX_GPU_COUNT;
  created = true;
  const made = receipt(fleet('create', ['--image', process.env.STADO_SANDBOX_GPU_IMAGE,
    '--cpus', process.env.STADO_SANDBOX_TEST_CPUS, '--memory-bytes', process.env.STADO_SANDBOX_TEST_MEMORY_BYTES,
    '--pids-limit', process.env.STADO_SANDBOX_TEST_PIDS_LIMIT, '--gpus', gpus]));
  assert.equal(made.result.Config.Labels['com.wisent.stado.sandbox.gpus'], gpus);
  assert.equal(made.result.HostConfig.Runtime, 'runsc');
  // The declared GPU command (a CUDA sample or `nvidia-smi -L`) runs on the
  // devices nvproxy hands the sandbox and is retained with its output.
  report.gpu_run = receipt(fleet('exec', ['--command', process.env.STADO_SANDBOX_GPU_COMMAND]));
  const removed = receipt(fleet('delete', ['--confirm', name]));
  created = false;
  assert.deepEqual(removed.result, { container: 'absent', workspace: 'absent' });
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  failure = error;
} finally {
  if (created) {
    const cleanup = fleet('delete', ['--confirm', name]);
    report.cleanup_succeeded = exitedCleanly(cleanup);
    if (!report.cleanup_succeeded) failure ??= new Error('GPU sandbox cleanup failed; see retained command receipt');
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;

function receiptless(answer) {
  assert.ok(exitedCleanly(answer), answer.stderr || answer.error?.message);
  return answer.stdout;
}
