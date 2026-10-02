import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { connect } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expectedRevision, snapshotSubject, verifyRevision } from '../native/subject.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, 'build/real-tests/database-transport');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
mkdirSync(home, { mode: 0o700 });
const config = join(home, 'stado.config.json');
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { started_at: new Date().toISOString(), commands: [], verdict: 'failed' };

function run(program, args, env = environment) {
  const result = spawnSync(program, args, { cwd: root, env, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 });
  report.commands.push({ program, args, exit_status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  return result;
}
function success(result) {
  assert.equal(result.status, 0, String(result.stderr || result.error || result.signal));
  return result.stdout.trim();
}

try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD'], process.env));
  report.expected_revision = expectedRevision(report.source_revision);
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.subject = snapshotSubject(process.env.STADO_BIN || 'stado', output);
  const binary = report.subject.path;
  report.binary_version = success(run(binary, ['--version']));
  try {
    report.native_revision = verifyRevision(report.binary_version, report.expected_revision);
  } catch (error) {
    report.verdict = 'blocked';
    throw error;
  }
  writeFileSync(config, '{}\n', { mode: 0o600 });
  success(run(binary, ['database', 'declare', 'example-ledger', '--engine', 'postgres', '--consumer', 'example-probe', '--json']));
  const before = readFileSync(config, 'utf8');
  writeFileSync(join(output, 'config-before.json'), before, { mode: 0o600 });
  const token = join(home, 'transport-token');
  writeFileSync(token, 'transport-test-no-listener', { mode: 0o600, flag: 'wx' });
  // TCP port zero cannot name a listening service. Observe the kernel's actual
  // refusal: its error number differs between operating systems.
  const probeResult = Promise.withResolvers();
  const probe = connect({ host: '127.0.0.1', port: 0 });
  probe.once('error', probeResult.resolve);
  probe.once('connect', () => {
    probe.destroy();
    probeResult.reject(new Error('TCP_PORT_ZERO_CONNECTED: expected the kernel to refuse port zero'));
  });
  const kernelError = await probeResult.promise;
  assert.ok(Number.isInteger(kernelError.errno), kernelError.message);
  report.kernel_error = { code: kernelError.code, errno: kernelError.errno, message: kernelError.message };
  const origin = 'http://127.0.0.1:0';
  const result = run(binary, ['database', 'resolve', 'example-ledger', '--consumer', 'example-probe', '--json'], {
    ...environment, WC_STORAGE_BACKEND: 'stado', WC_STADO_STORAGE_URL: origin,
    WC_STADO_STORAGE_TOKEN_FILE: token, WC_STADO_STORAGE_CA_FILE: '',
    NO_PROXY: '127.0.0.1', no_proxy: '127.0.0.1',
  });
  const after = readFileSync(config, 'utf8');
  writeFileSync(join(output, 'config-after.json'), after, { mode: 0o600 });
  assert.equal(result.status, 69, `${result.stdout}\n${result.stderr}`);
  assert.ok(result.stderr.includes('infra_down'), result.stderr);
  assert.ok(result.stderr.includes(`os error ${Math.abs(kernelError.errno)}`), result.stderr);
  assert.ok(result.stderr.includes(origin), result.stderr);
  assert.equal(after, before, 'a refused resolution changed the declared database');
  report.configuration_unchanged = true;
  report.verdict = 'passed';
} catch (error) {
  report.error = { code: error.code, message: error.message, stack: error.stack };
  process.exitCode = 1;
} finally {
  try { rmSync(home, { recursive: true, force: true }); }
  catch (error) { report.cleanup_error = error.message; report.verdict = 'failed'; process.exitCode = 1; }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
