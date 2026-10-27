import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';
import { exitedCleanly } from '../native/process.mjs';

// Actual remote gVisor lifecycle through Stado. No simulated provider or host.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const area = join(root, '.build', 'sandbox');
mkdirSync(area, { recursive: true });
const output = mkdtempSync(join(area, 'lifecycle-'));
const report = { result: 'blocked', started_at: new Date().toISOString(), commands: [] };
const name = `qualification-${randomUUID()}`;
let binary;
let host;
let created = false;
const timed = `${name}-lifetime`;
let timedCreated = false;
const copy = `${name}-copy`;
let copyCreated = false;
const fenced = `${name}-egress`;
let fencedCreated = false;
let failure;
function run(args) {
  const answer = spawnSync(binary, args, { cwd: root, env: process.env, encoding: 'utf8' });
  report.commands.push({ program: binary, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(exitedCleanly(answer), answer.stderr || answer.error?.message);
  return answer.stdout;
}
function fleet(verb, extra = [], named = true) {
  return run(['sandbox', 'fleet', verb, ...(named ? [name] : []), '--host', host, ...extra]);
}
function receipt(answer) {
  const document = JSON.parse(success(answer));
  assert.equal(document.ok, true, JSON.stringify(document));
  assert.equal(document.host, host);
  return document;
}
function refused(answer, message) {
  assert.ok(!exitedCleanly(answer), 'operation unexpectedly succeeded');
  assert.ok(`${answer.stderr}${answer.stdout}`.includes(message), `${message}: ${answer.stderr}`);
}
function kernelLimits(document, cpus, memory, pids) {
  const limits = document.kernel_limits;
  assert.equal(limits.memory_bytes, memory, 'the running sandbox cgroup must enforce the RAM ceiling');
  assert.equal(limits.swap_bytes, '0', 'the running sandbox cgroup must disable swap');
  assert.equal(limits.pids_limit, pids, 'the running sandbox cgroup must enforce the process ceiling');
  assert.notEqual(limits.cpu_quota, 'max', 'the running sandbox must not have unlimited CPU');
  assert.equal(BigInt(limits.cpu_quota), BigInt(limits.cpu_period) * BigInt(cpus));
}
try {
  const required = ['STADO_BIN', 'STADO_TEST_SOURCE_REVISION', 'STADO_SANDBOX_TEST_HOST',
    'STADO_SANDBOX_TEST_IMAGE', 'STADO_SANDBOX_TEST_CPUS', 'STADO_SANDBOX_TEST_MEMORY_BYTES',
    'STADO_SANDBOX_TEST_PIDS_LIMIT', 'STADO_SANDBOX_TEST_NANO_CPUS',
    'STADO_SANDBOX_TEST_UPDATED_CPUS', 'STADO_SANDBOX_TEST_UPDATED_MEMORY_BYTES',
    'STADO_SANDBOX_TEST_UPDATED_PIDS_LIMIT', 'STADO_SANDBOX_TEST_UPDATED_NANO_CPUS',
    'STADO_SANDBOX_TEST_STOP_AFTER_SECONDS', 'STADO_SANDBOX_TEST_CREDENTIAL',
    'STADO_SANDBOX_TEST_PROXY_IMAGE', 'STADO_SANDBOX_TEST_EGRESS_HOST', 'STADO_SANDBOX_TEST_DENIED_HOST'];
  const missing = required.filter(key => !process.env[key]);
  assert.deepEqual(missing, [], `real lifecycle requires explicit inputs: ${missing.join(', ')}`);
  report.subject = snapshotSubject(process.env.STADO_BIN, output);
  binary = report.subject.path;
  host = process.env.STADO_SANDBOX_TEST_HOST;
  report.source_revision = process.env.STADO_TEST_SOURCE_REVISION;
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  verifyRevision(success(run(['--version'])), report.source_revision);
  const image = process.env.STADO_SANDBOX_TEST_IMAGE;
  const cpus = process.env.STADO_SANDBOX_TEST_CPUS;
  const memory = process.env.STADO_SANDBOX_TEST_MEMORY_BYTES;
  const pids = process.env.STADO_SANDBOX_TEST_PIDS_LIMIT;
  const resources = ['--image', image, '--cpus', cpus, '--memory-bytes', memory, '--pids-limit', pids];
  receipt(fleet('inventory', [], false));
  report.result = 'failed';
  refused(fleet('create', ['--image', 'unversioned:latest', '--cpus', cpus, '--memory-bytes', memory, '--pids-limit', pids]), 'immutable');
  refused(fleet('create', ['--image', image, '--cpus', '0', '--memory-bytes', memory, '--pids-limit', pids]), '--cpus');
  created = true;
  const made = receipt(fleet('create', resources));
  report.created = made.result;
  const initial = receipt(fleet('show'));
  const observed = initial.result;
  kernelLimits(initial, cpus, memory, pids);
  assert.equal(observed.State.Running, true);
  assert.equal(observed.HostConfig.Runtime, 'runsc');
  assert.equal(String(observed.HostConfig.Memory), memory);
  assert.equal(String(observed.HostConfig.MemorySwap), memory);
  assert.equal(String(observed.HostConfig.NanoCpus), process.env.STADO_SANDBOX_TEST_NANO_CPUS);
  assert.equal(String(observed.HostConfig.PidsLimit), pids);
  assert.equal(observed.HostConfig.ReadonlyRootfs, true);
  assert.equal(observed.HostConfig.NetworkMode, 'none');
  assert.equal(observed.HostConfig.Privileged, false);
  assert.ok(observed.HostConfig.CapDrop.includes('ALL'));
  assert.ok(observed.HostConfig.SecurityOpt.includes('no-new-privileges'));
  assert.ok(observed.Mounts.every(mount => mount.Type !== 'bind'));
  assert.equal(observed.Mounts.find(mount => mount.Destination === '/workspace')?.Type, 'volume');
  // Without a declared lifetime the sandbox runs until stopped.
  assert.equal(observed.Config.Labels['com.wisent.stado.sandbox.stop-after-seconds'], '');
  assert.deepEqual(observed.Config.Cmd, ['-c', 'exec sleep infinity']);
  refused(fleet('create', resources), 'already exists');
  const updated = {
    cpus: process.env.STADO_SANDBOX_TEST_UPDATED_CPUS,
    memory: process.env.STADO_SANDBOX_TEST_UPDATED_MEMORY_BYTES,
    pids: process.env.STADO_SANDBOX_TEST_UPDATED_PIDS_LIMIT,
  };
  assert.notDeepEqual(updated, { cpus, memory, pids }, 'update must exercise changed limits');
  receipt(fleet('update', ['--cpus', updated.cpus, '--memory-bytes', updated.memory, '--pids-limit', updated.pids]));
  const changed = receipt(fleet('show'));
  kernelLimits(changed, updated.cpus, updated.memory, updated.pids);
  const resized = changed.result.HostConfig;
  assert.equal(String(resized.Memory), updated.memory);
  assert.equal(String(resized.MemorySwap), updated.memory);
  assert.equal(String(resized.PidsLimit), updated.pids);
  assert.equal(String(resized.NanoCpus), process.env.STADO_SANDBOX_TEST_UPDATED_NANO_CPUS);
  const token = randomUUID();
  receipt(fleet('exec', ['--command', `printf '%s' '${token}' > /workspace/proof && test ! -e /var/run/docker.sock && test ! -w /etc && mkfifo /workspace/barrier`]));
  assert.ok(receipt(fleet('exec', ['--command', 'cat /workspace/proof'])).stdout.includes(token));
  refused(fleet('exec', ['--command', 'exit 17']), '17');
  // A granted credential field reaches the sandboxed command's environment:
  // the digest the command computes inside the sandbox equals the one the
  // same field gives `credentials run` here, and only digests are kept.
  const credential = process.env.STADO_SANDBOX_TEST_CREDENTIAL;
  const digestOf = 'printf %s "$SANDBOX_CREDENTIAL" | sha256sum';
  const local = success(run(['credentials', 'run', '--env', `SANDBOX_CREDENTIAL=${credential}`, '--', '/bin/sh', '-c', digestOf]));
  const inside = receipt(fleet('exec', ['--credential', `SANDBOX_CREDENTIAL=${credential}`, '--command', `test -n "$SANDBOX_CREDENTIAL" && ${digestOf}`]));
  assert.equal(inside.stdout.trim().split(/\s+/)[0], local.trim().split(/\s+/)[0], 'the sandboxed command did not receive the granted field');
  refused(fleet('exec', ['--credential', 'MALFORMED', '--command', 'true']), 'NAME=ITEM#FIELD');
  receipt(fleet('exec', ['--command', 'test -z "${SANDBOX_CREDENTIAL:-}"']));
  const execution = receipt(fleet('exec', ['--command', `printf '%s' '${token}' > /workspace/background && printf '%s' '${token}' && printf '%s\n' '${token}' > /workspace/barrier`, '--detach']));
  assert.match(execution.result.execution, /^[a-f\d-]+$/);
  // A FIFO waits on the actual background writer, not on an invented interval.
  receipt(fleet('exec', ['--command', `read value < /workspace/barrier && test "$value" = '${token}' && test "$(cat /workspace/background)" = '${token}'`]));
  const logs = receipt(fleet('logs', ['--execution', execution.result.execution]));
  assert.ok(logs.stdout.includes(token), 'retained background output must match the completed workload');
  // The workload's own use shows in the sandbox's counters.
  const used = receipt(fleet('usage')).result;
  for (const counter of ['cpu_usage_usec', 'memory_peak_bytes', 'memory_current_bytes', 'processes', 'workspace_kib']) {
    assert.ok(Number.isInteger(used[counter]) && used[counter] > 0, `${counter}: ${JSON.stringify(used)}`);
  }
  assert.ok(used.memory_peak_bytes >= used.memory_current_bytes, JSON.stringify(used));
  assert.equal(used.since, receipt(fleet('show')).result.State.StartedAt);
  // A running sandbox is not forked: its workspace could change mid-copy.
  refused(fleet('fork', ['--to', copy]), 'is running');
  refused(fleet('fork', ['--to', name]), '--to');
  assert.equal(receipt(fleet('stop')).result.State.Status, 'exited');
  refused(fleet('exec', ['--command', 'true']), 'not running');
  refused(fleet('usage'), 'is not running');
  // The fork carries the workspace byte for byte and the enforced limits.
  copyCreated = true;
  const forked = receipt(fleet('fork', ['--to', copy]));
  assert.match(forked.stdout, /workspace digest: [a-f\d]+/);
  assert.equal(forked.result.State.Running, true);
  kernelLimits(forked, updated.cpus, updated.memory, updated.pids);
  const copyFleet = (verb, extra = []) => run(['sandbox', 'fleet', verb, copy, '--host', host, ...extra]);
  assert.ok(receipt(copyFleet('exec', ['--command', 'cat /workspace/proof'])).stdout.includes(token));
  assert.ok(receipt(copyFleet('exec', ['--command', 'cat /workspace/background'])).stdout.includes(token));
  refused(fleet('fork', ['--to', copy]), 'already exists');
  assert.deepEqual(receipt(copyFleet('delete', ['--confirm', copy])).result, { container: 'absent', workspace: 'absent' });
  copyCreated = false;
  const resumed = receipt(fleet('resume'));
  assert.equal(resumed.result.State.Running, true);
  kernelLimits(resumed, updated.cpus, updated.memory, updated.pids);
  assert.ok(receipt(fleet('exec', ['--command', 'cat /workspace/proof'])).stdout.includes(token));
  refused(fleet('delete', ['--confirm', 'different-name']), '--confirm');
  const removed = receipt(fleet('delete', ['--confirm', name]));
  created = false;
  assert.deepEqual(removed.result, { container: 'absent', workspace: 'absent' });
  refused(fleet('show'), 'No such');
  report.final_state = removed.result;
  // A declared lifetime stops the sandbox by itself and keeps its workspace.
  const lifetime = process.env.STADO_SANDBOX_TEST_STOP_AFTER_SECONDS;
  const timedFleet = (verb, extra = []) => run(['sandbox', 'fleet', verb, timed, '--host', host, ...extra]);
  refused(timedFleet('create', [...resources, '--stop-after-seconds', '0']), '--stop-after-seconds');
  timedCreated = true;
  const born = receipt(timedFleet('create', [...resources, '--stop-after-seconds', lifetime]));
  assert.equal(born.result.Config.Labels['com.wisent.stado.sandbox.stop-after-seconds'], lifetime);
  assert.deepEqual(born.result.Config.Cmd, ['-c', `exec sleep ${lifetime}`]);
  receipt(timedFleet('exec', ['--command', `printf '%s' '${token}' > /workspace/kept`]));
  // A read from a FIFO nobody writes ends only when the lifetime stops
  // every process in the sandbox: the wait is the sandbox's own stop.
  const held = timedFleet('exec', ['--command', 'mkfifo /workspace/never && read value < /workspace/never']);
  assert.ok(!exitedCleanly(held), 'a held execution outlived the declared lifetime');
  assert.equal(receipt(timedFleet('show')).result.State.Status, 'exited');
  assert.equal(receipt(timedFleet('resume')).result.State.Running, true);
  assert.ok(receipt(timedFleet('exec', ['--command', 'cat /workspace/kept'])).stdout.includes(token));
  const timedRemoved = receipt(timedFleet('delete', ['--confirm', timed]));
  timedCreated = false;
  assert.deepEqual(timedRemoved.result, { container: 'absent', workspace: 'absent' });
  // Declared egress reaches its host through the sandbox's own proxy and
  // nothing else: another host is refused by the proxy, and traffic that
  // ignores the proxy has no route out of the internal network.
  const allowed = process.env.STADO_SANDBOX_TEST_EGRESS_HOST;
  const denied = process.env.STADO_SANDBOX_TEST_DENIED_HOST;
  const proxyImage = process.env.STADO_SANDBOX_TEST_PROXY_IMAGE;
  const fencedFleet = (verb, extra = []) => run(['sandbox', 'fleet', verb, fenced, '--host', host, ...extra]);
  refused(fencedFleet('create', [...resources, '--egress', allowed]), '--egress-proxy-image');
  refused(fencedFleet('create', [...resources, '--egress', 'Not_A_Host', '--egress-proxy-image', proxyImage]), 'no host name');
  refused(fencedFleet('create', [...resources, '--egress-proxy-image', proxyImage]), 'none is declared');
  fencedCreated = true;
  const fencedMade = receipt(fencedFleet('create', [...resources, '--egress', allowed, '--egress-proxy-image', proxyImage]));
  assert.equal(fencedMade.result.Config.Labels['com.wisent.stado.sandbox.egress'], allowed);
  assert.equal(fencedMade.result.HostConfig.NetworkMode, `stado-sandbox-${fenced}-egress`);
  receipt(fencedFleet('exec', ['--command', `wget -q -O /dev/null http://${allowed}/`]));
  refused(fencedFleet('exec', ['--command', `wget -q -O /dev/null http://${denied}/`]), 'exit');
  refused(fencedFleet('exec', ['--command', `env -u http_proxy -u HTTP_PROXY wget -q -O /dev/null http://${allowed}/`]), 'exit');
  assert.equal(receipt(fencedFleet('stop')).result.State.Status, 'exited');
  assert.equal(receipt(fencedFleet('resume')).result.State.Running, true);
  receipt(fencedFleet('exec', ['--command', `wget -q -O /dev/null http://${allowed}/`]));
  const fencedRemoved = receipt(fencedFleet('delete', ['--confirm', fenced]));
  fencedCreated = false;
  assert.deepEqual(fencedRemoved.result, { container: 'absent', workspace: 'absent' });
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  failure = error;
} finally {
  if (fencedCreated) {
    const cleanup = run(['sandbox', 'fleet', 'delete', fenced, '--host', host, '--confirm', fenced]);
    report.fenced_cleanup_succeeded = exitedCleanly(cleanup);
    if (!report.fenced_cleanup_succeeded) failure ??= new Error('egress sandbox cleanup failed; see retained command receipt');
  }
  if (copyCreated) {
    const cleanup = run(['sandbox', 'fleet', 'delete', copy, '--host', host, '--confirm', copy]);
    report.copy_cleanup_succeeded = exitedCleanly(cleanup);
    if (!report.copy_cleanup_succeeded) failure ??= new Error('forked sandbox cleanup failed; see retained command receipt');
  }
  if (timedCreated) {
    const cleanup = run(['sandbox', 'fleet', 'delete', timed, '--host', host, '--confirm', timed]);
    report.timed_cleanup_succeeded = exitedCleanly(cleanup);
    if (!report.timed_cleanup_succeeded) failure ??= new Error('lifetime sandbox cleanup failed; see retained command receipt');
  }
  if (created) {
    const cleanup = fleet('delete', ['--confirm', name]);
    report.cleanup_succeeded = exitedCleanly(cleanup);
    if (!report.cleanup_succeeded) failure ??= new Error('sandbox cleanup failed; see retained command receipt');
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
