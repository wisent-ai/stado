// Exercises the installed Stado HTTP parser through an isolated enrollment listener.
// Required: STADO_BIN, STADO_TEST_HEAD_BYTES (the candidate's head budget),
// STADO_TEST_PORT (use the OS-assigned port declaration for an isolated run).
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { createConnection } from 'node:net';
import { networkInterfaces } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const parent = join(root, 'build', 'http-header-boundary');
mkdirSync(parent, { recursive: true });
const output = mkdtempSync(join(parent, 'run-'));
const home = join(output, 'home');
mkdirSync(home);
const report = { started_at: new Date().toISOString(), verdict: 'blocked', commands: [], cases: [] };
let child;
let exited;

function command(program, args) {
  const result = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr });
  if (result.error) throw result.error;
  assert.equal(result.signal, null, `${program} was interrupted`);
  assert.notEqual(result.status, null, `${program} did not exit normally`);
  assert.ok(!result.status, result.stderr);
  return result.stdout.trim();
}

async function exchange(endpoint, request) {
  const socket = createConnection({ host: endpoint.hostname, port: endpoint.port });
  const received = [];
  socket.on('data', bytes => received.push(bytes));
  try {
    await once(socket, 'connect');
    const closed = once(socket, 'close');
    socket.end(request);
    await closed;
    return Buffer.concat(received).toString();
  } finally {
    socket.destroy();
  }
}

try {
  const binary = realpathSync(required('STADO_BIN'));
  const budget = Number(required('STADO_TEST_HEAD_BYTES'));
  assert.ok(Number.isSafeInteger(budget), 'STADO_TEST_HEAD_BYTES must be whole bytes');
  const loopback = Object.values(networkInterfaces()).flat().find(address => address.internal && address.family === 'IPv4');
  assert.ok(loopback, 'an IPv4 loopback interface is required');
  report.source_revision = command('git', ['rev-parse', 'HEAD']);
  report.checkout_status = command('git', ['status', '--porcelain']);
  report.binary = binary;
  report.binary_sha256 = createHash('sha256').update(readFileSync(binary)).digest('hex');
  report.binary_version = command(binary, ['--version']);
  report.test_sha256 = createHash('sha256').update(readFileSync(fileURLToPath(import.meta.url))).digest('hex');
  report.head_budget = budget;
  const args = ['serve', '--api', '--enrollment-only', '--bind', loopback.address, '--port', required('STADO_TEST_PORT')];
  const launch = { program: binary, args, stdout: '', stderr: '' };
  report.commands.push(launch);
  child = spawn(binary, args, {
    cwd: root,
    env: { ...process.env, HOME: home, STADO_CONFIG: join(home, 'config.json'), WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(home, 'store') },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  exited = new Promise(resolveExit => child.once('close', (status, signal) => {
    Object.assign(launch, { status, signal });
    resolveExit();
  }));
  child.stdout.on('data', bytes => { launch.stdout += bytes; });
  const endpoint = await new Promise((resolveEndpoint, reject) => {
    child.once('error', reject);
    child.once('close', () => reject(new Error(`Stado exited before serving: ${launch.stderr}`)));
    child.stderr.on('data', bytes => {
      launch.stderr += bytes;
      const match = launch.stderr.match(/enrollment-only listener on (http:\/\/[^\s]+)/);
      if (match) {
        const [, url] = match;
        resolveEndpoint(new URL(url));
      }
    });
  });
  report.endpoint = endpoint.href;
  const prefix = `POST /header-boundary HTTP/1.1\r\nHost: ${endpoint.host}\r\nConnection: close\r\nContent-Length: ${Buffer.byteLength('body')}\r\nX-Padding: `;
  const ending = '\r\n\r\n';
  const padding = 'x'.repeat(budget - Buffer.byteLength(prefix + ending));
  const exact = prefix + padding + ending;
  const cases = [
    { name: 'exact-head-with-coalesced-body', request: exact + 'body', status: 'HTTP/1.1 404 ' },
    { name: 'oversized-complete-head', request: prefix + padding + 'x' + ending + 'body', status: 'HTTP/1.1 400 ', reason: 'HTTP request head too large' },
    { name: 'full-budget-without-terminator', request: exact.replace(ending, 'xxxx'), status: 'HTTP/1.1 400 ', reason: 'HTTP request head too large' },
  ];
  report.verdict = 'failed';
  for (const entry of cases) {
    const response = await exchange(endpoint, entry.request);
    const observation = { name: entry.name, request_bytes: Buffer.byteLength(entry.request), response, verdict: 'failed' };
    report.cases.push(observation);
    if (response.startsWith(entry.status) && (!entry.reason || response.includes(entry.reason))) {
      observation.verdict = 'passed';
    }
  }
  assert.ok(report.cases.every(entry => entry.verdict === 'passed'), JSON.stringify(report.cases));
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack ?? error);
  throw error;
} finally {
  if (child && child.exitCode === null && child.signalCode === null) child.kill();
  if (exited) await exited;
  rmSync(home, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t') + '\n');
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
