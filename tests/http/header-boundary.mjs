// Exercises the installed Stado HTTP parser through an isolated enrollment listener.
// Required: STADO_BIN, STADO_TEST_HEAD_BYTES (the fixture's declared head budget),
// STADO_TEST_PORT (use the OS-assigned port declaration for an isolated run).
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
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

async function start(binary, args, env) {
  const launch = { program: binary, args, request_limits: env.WC_DASHBOARD_REQUEST_LIMITS, stdout: '', stderr: '' };
  report.commands.push(launch);
  child = spawn(binary, args, { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
  exited = new Promise(resolveExit => child.once('close', (status, signal) => {
    Object.assign(launch, { status, signal });
    resolveExit();
  }));
  child.stdout.on('data', bytes => { launch.stdout += bytes; });
  const endpoint = await new Promise((resolveEndpoint, reject) => {
    child.once('error', reject);
    child.once('close', () => resolveEndpoint(null));
    child.stderr.on('data', bytes => {
      launch.stderr += bytes;
      const match = launch.stderr.match(/(?:enrollment-only listener on|listening on) (http:\/\/[^\s]+)/);
      if (match) {
        const [, url] = match;
        resolveEndpoint(new URL(url));
      }
    });
  });
  return { endpoint, launch };
}

async function stop() {
  if (child && child.exitCode === null && child.signalCode === null) child.kill();
  if (exited) await exited;
  child = undefined;
  exited = undefined;
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
  const environment = { ...process.env, HOME: home, STADO_CONFIG: join(home, 'config.json'), WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(home, 'store') };
  delete environment.WC_DASHBOARD_REQUEST_LIMITS;
  const limits = { head_bytes: budget, body_bytes: budget + Buffer.byteLength('body'), registry_import_bytes: Buffer.byteLength('import-body') };
  const consoleArgs = ['config', 'show', '--json'];
  const consoleInput = 'input';
  limits.operator_console = {
    argument_count: consoleArgs.length,
    argument_bytes: Math.max(...consoleArgs.map(value => Buffer.byteLength(value))),
    input_bytes: Buffer.byteLength(consoleInput),
    request_bytes: Buffer.byteLength(JSON.stringify({ args: consoleArgs, input: consoleInput, stdin: consoleInput, confirmation: 'RUN_MUTATION' })),
  };
  const joinReport = { hostname: 'enrollment-boundary', os: process.platform, arch: process.arch, destination: `operator@${loopback.address}`, installed_key_fingerprint: '', ssh_listening: false };
  const fieldBytes = Math.max(...Object.values(joinReport).filter(value => typeof value === 'string').map(value => Buffer.byteLength(value)));
  const joinCases = [
    { name: 'join-field-over-limit', body: JSON.stringify({ ...joinReport, os: 'x'.repeat(fieldBytes) + 'x' }), status: 'HTTP/1.1 400 ', reason: 'fleet_join.field_bytes' },
    { name: 'join-multibyte-field-over-limit', body: JSON.stringify({ ...joinReport, os: 'é'.repeat(fieldBytes) }), status: 'HTTP/1.1 400 ', reason: 'fleet_join.field_bytes' },
    { name: 'join-fingerprint-over-limit', body: JSON.stringify({ ...joinReport, installed_key_fingerprint: 'x'.repeat(fieldBytes) + 'x' }), status: 'HTTP/1.1 400 ', reason: 'installed_key_fingerprint' },
    { name: 'join-empty-hostname', body: JSON.stringify({ ...joinReport, hostname: '' }), status: 'HTTP/1.1 400 ', reason: 'hostname' },
    { name: 'join-invalid-hostname', body: JSON.stringify({ ...joinReport, hostname: '../escape' }), status: 'HTTP/1.1 400 ', reason: 'hostname' },
    { name: 'join-missing-architecture', body: JSON.stringify({ ...joinReport, arch: null }), status: 'HTTP/1.1 400 ', reason: 'arch' },
    { name: 'join-invalid-json', body: '{', status: 'HTTP/1.1 400 ', reason: 'not JSON' },
  ];
  limits.fleet_join = { field_bytes: fieldBytes, request_bytes: Math.max(...joinCases.map(entry => Buffer.byteLength(entry.body))) };
  const joinAtLimit = JSON.stringify(joinReport).padEnd(limits.fleet_join.request_bytes, ' ');
  joinCases.push(
    { name: 'join-at-declared-bounds-needs-token', body: joinAtLimit, status: 'HTTP/1.1 401 ' },
    { name: 'join-request-over-limit', body: joinAtLimit + ' ', status: 'HTTP/1.1 413 ', reason: `accepts at most ${limits.fleet_join.request_bytes} bytes` },
    { name: 'join-query-request-over-limit', query: 'source=bootstrap', body: joinAtLimit + ' ', status: 'HTTP/1.1 413 ', reason: `accepts at most ${limits.fleet_join.request_bytes} bytes` },
  );
  report.request_limits = limits;
  const refusals = [
    { name: 'missing-declaration', value: undefined, reason: 'API request limits are not declared' },
    { name: 'invalid-json', value: '{', reason: 'WC_DASHBOARD_REQUEST_LIMITS cannot be read' },
    { name: 'missing-bound', value: JSON.stringify({ head_bytes: budget }), reason: 'body_bytes' },
    { name: 'invalid-bound', value: JSON.stringify({ ...limits, body_bytes: false }), reason: 'positive whole-byte' },
    { name: 'unknown-bound', value: JSON.stringify({ ...limits, mistyped_bound: budget }), reason: 'unknown field' },
    { name: 'missing-console-bound', value: JSON.stringify({ ...limits, operator_console: {} }), reason: 'argument_count' },
    { name: 'missing-join-bound', value: JSON.stringify({ ...limits, fleet_join: {} }), reason: 'request_bytes' },
  ];
  report.verdict = 'failed';
  for (const entry of refusals) {
    const env = { ...environment };
    if (entry.value !== undefined) env.WC_DASHBOARD_REQUEST_LIMITS = entry.value;
    const run = await start(binary, args, env);
    await stop();
    const passed = run.endpoint === null && run.launch.signal === null && Boolean(run.launch.status) && run.launch.stderr.includes(entry.reason);
    report.cases.push({ name: entry.name, verdict: passed ? 'passed' : 'failed' });
  }
  const run = await start(binary, args, { ...environment, WC_DASHBOARD_REQUEST_LIMITS: JSON.stringify(limits) });
  const { endpoint } = run;
  assert.ok(endpoint, `valid declaration did not start a listener: ${run.launch.stderr}`);
  report.endpoint = endpoint.href;
  const prefix = `POST /header-boundary HTTP/1.1\r\nHost: ${endpoint.host}\r\nConnection: close\r\nContent-Length: ${Buffer.byteLength('body')}\r\nX-Padding: `;
  const ending = '\r\n\r\n';
  const padding = 'x'.repeat(budget - Buffer.byteLength(prefix + ending));
  const exact = prefix + padding + ending;
  const bodyAtLimit = 'x'.repeat(limits.body_bytes);
  const importAtLimit = 'x'.repeat(limits.registry_import_bytes);
  const bodyRequest = (path, body) => `POST ${path} HTTP/1.1\r\nHost: ${endpoint.host}\r\nConnection: close\r\nContent-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`;
  const cases = [
    { name: 'exact-head-with-coalesced-body', request: exact + 'body', status: 'HTTP/1.1 404 ' },
    { name: 'oversized-complete-head', request: prefix + padding + 'x' + ending + 'body', status: 'HTTP/1.1 400 ', reason: 'HTTP request head too large' },
    { name: 'full-budget-without-terminator', request: exact.replace(ending, 'xxxx'), status: 'HTTP/1.1 400 ', reason: 'HTTP request head too large' },
    { name: 'body-at-own-limit-larger-than-head-budget', request: bodyRequest('/header-boundary', bodyAtLimit), status: 'HTTP/1.1 404 ' },
    { name: 'oversized-ordinary-body', request: bodyRequest('/header-boundary', bodyAtLimit + 'x'), status: 'HTTP/1.1 413 ', reason: `accepts at most ${limits.body_bytes} bytes` },
    { name: 'import-at-own-limit', request: bodyRequest('/api/registry/import', importAtLimit), status: 'HTTP/1.1 404 ' },
    { name: 'oversized-import-body', request: bodyRequest('/api/registry/import', importAtLimit + 'x'), status: 'HTTP/1.1 413 ', reason: `accepts at most ${limits.registry_import_bytes} bytes` },
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
  for (const entry of joinCases) {
    const target = new URL('/api/fleet/join', endpoint);
    if (entry.query) target.search = entry.query;
    const request = `POST ${target.pathname}${target.search} HTTP/1.1\r\nHost: ${endpoint.host}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(entry.body)}\r\n\r\n${entry.body}`;
    const response = await exchange(endpoint, request);
    const enrollmentWritten = existsSync(join(home, 'store', 'enrollments'));
    const passed = response.startsWith(entry.status) && (!entry.reason || response.includes(entry.reason)) && !enrollmentWritten;
    report.cases.push({ name: entry.name, request_bytes: Buffer.byteLength(request), response, enrollment_written: enrollmentWritten, verdict: passed ? 'passed' : 'failed' });
  }
  await stop();
  const consoleRun = await start(binary, args.filter(value => value !== '--enrollment-only'), { ...environment, WC_DASHBOARD_REQUEST_LIMITS: JSON.stringify(limits) });
  assert.ok(consoleRun.endpoint, `operator API did not start: ${consoleRun.launch.stderr}`);
  report.console_endpoint = consoleRun.endpoint.href;
  const consoleBody = JSON.stringify({ args: consoleArgs, input: consoleInput, stdin: consoleInput, confirmation: 'RUN_MUTATION' });
  const consoleCases = [
    { name: 'console-too-many-arguments', body: JSON.stringify({ args: [...consoleArgs, '--json'] }), status: 'HTTP/1.1 400 ', reason: 'argument_count' },
    { name: 'console-argument-too-long', body: JSON.stringify({ args: ['config', 'show', '--invalid-argument'] }), status: 'HTTP/1.1 400 ', reason: 'argument_bytes' },
    { name: 'console-input-too-long', body: JSON.stringify({ args: consoleArgs, input: consoleInput + 'x' }), status: 'HTTP/1.1 400 ', reason: 'input_bytes' },
    { name: 'console-stdin-too-long', body: JSON.stringify({ args: consoleArgs, stdin: consoleInput + 'x' }), status: 'HTTP/1.1 400 ', reason: 'input_bytes' },
    { name: 'console-nul-argument', body: JSON.stringify({ args: ['config', '\0'] }), status: 'HTTP/1.1 400 ', reason: 'NUL' },
    { name: 'console-unconfirmed-mutation', body: JSON.stringify({ args: ['config', 'unset', 'a'] }), status: 'HTTP/1.1 403 ', reason: 'RUN_MUTATION' },
    { name: 'console-at-request-limit', body: consoleBody.padEnd(limits.operator_console.request_bytes, ' '), status: 'HTTP/1.1 200 ', execution: true },
    { name: 'console-over-request-limit', body: consoleBody.padEnd(limits.operator_console.request_bytes, ' ') + ' ', status: 'HTTP/1.1 413 ', reason: `accepts at most ${limits.operator_console.request_bytes} bytes` },
  ];
  for (const entry of consoleCases) {
    const request = `POST /api/operator/run HTTP/1.1\r\nHost: ${consoleRun.endpoint.host}\r\nConnection: close\r\nX-Stado-Action: operator-command\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(entry.body)}\r\n\r\n${entry.body}`;
    const response = await exchange(consoleRun.endpoint, request);
    const observation = { name: entry.name, request_bytes: Buffer.byteLength(request), response, verdict: 'failed' };
    report.cases.push(observation);
    let passed = response.startsWith(entry.status) && (!entry.reason || response.includes(entry.reason));
    if (passed && entry.execution) {
      const [, body] = response.split('\r\n\r\n');
      const receipt = JSON.parse(body);
      passed = Number.isInteger(receipt.exit_code) && !receipt.exit_code && typeof receipt.stdout === 'string';
      if (passed) observation.configuration = JSON.parse(receipt.stdout);
    }
    if (passed) observation.verdict = 'passed';
  }
  assert.ok(report.cases.every(entry => entry.verdict === 'passed'), JSON.stringify(report.cases));
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack ?? error);
  throw error;
} finally {
  await stop();
  rmSync(home, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t') + '\n');
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
