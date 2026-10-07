// Runs the real proxy through its CLI and TCP sockets. No browser or provider substitute.
// Required: STADO_BIN, STADO_TEST_SOURCE_REVISION, STADO_EGRESS_TEST_INTERFACE.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { command, proxy, receiver, transmit } from './support.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
};
const binary = resolve(required('STADO_BIN'));
const revision = required('STADO_TEST_SOURCE_REVISION');
const iface = required('STADO_EGRESS_TEST_INTERFACE');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(root, '.build', 'egress-headers-'));
const home = join(output, 'home');
mkdirSync(home);
const options = { cwd: root, env: { ...process.env, HOME: home, STADO_CONFIG: join(home, 'config.json') } };
const report = { revision, binary, interface: iface, started_at: new Date().toISOString(), commands: [], cases: [], result: 'failed' };
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
let upstream;
let running;
try {
  const source = command(report, 'git', ['rev-parse', 'HEAD'], options);
  assert.equal(source.stdout.trim(), revision, 'declared candidate revision must match this checkout');
  report.binary_sha256 = hash(binary);
  report.test_sha256 = hash(fileURLToPath(import.meta.url));
  report.support_sha256 = hash(join(dirname(fileURLToPath(import.meta.url)), 'support.mjs'));
  report.version = command(report, binary, ['--version'], options).stdout;
  const help = command(report, binary, ['egress', 'mobile', 'serve', '--help'], options);
  assert.ok(help.stdout.includes('--max-header-bytes'), 'candidate has no declared header boundary');
  const addresses = networkInterfaces();
  const sourceAddress = addresses[iface]?.find(address => address.family === 'IPv4' && !address.internal);
  assert.ok(sourceAddress, `${iface} must have a real non-loopback IPv4 address`);
  const loopback = Object.values(addresses).flat().find(address => address.family === 'IPv4' && address.internal);
  assert.ok(loopback, 'a loopback IPv4 address is required');
  upstream = await receiver(sourceAddress.address);
  report.upstream = upstream.address;
  const destination = `${upstream.address.address}:${upstream.address.port}`;
  const payload = Buffer.from('payload beyond the header budget');
  const header = Buffer.from(`POST http://${destination}/egress HTTP/1.1\r\nHost: ${destination}\r\nContent-Length: ${payload.length}\r\n\r\n`);
  const forwarded = Buffer.from(`POST /egress HTTP/1.1\r\nHost: ${destination}\r\nContent-Length: ${payload.length}\r\n\r\n`);
  const connect = Buffer.from(`CONNECT ${destination} HTTP/1.1\r\nHost: ${destination}\r\n\r\n`);
  const reservation = await receiver(loopback.address);
  const port = String(reservation.address.port);
  await reservation.close();
  const args = ['egress', 'mobile', 'serve', '--interface', iface, '--bind', loopback.address, '--port', port];
  const absent = command(report, binary, args, options);
  assert.ok(absent.status, 'missing header declaration must refuse without starting a listener');
  assert.ok(absent.stderr.includes('--max-header-bytes'), 'missing declaration must be named');
  report.cases.push({ name: 'missing-declaration', result: 'passed' });
  running = await proxy(report, binary, [...args, '--max-header-bytes', String(header.length)], options);
  const accepted = await transmit(running.address, [Buffer.concat([header, payload])]);
  assert.deepEqual(accepted.errors, []);
  assert.deepEqual(upstream.connections.map(record => record.bytes), [Buffer.concat([forwarded, payload])],
    'exact-boundary header and its body must reach the actual upstream intact');
  report.cases.push({ name: 'exact-boundary-with-body', result: 'passed' });
  const count = upstream.connections.length;
  const excess = Buffer.from(header.toString().replace('/egress ', '/egress-extra '));
  const refused = await transmit(running.address, [excess]);
  assert.equal(upstream.connections.length, count, 'oversized header must not connect upstream');
  assert.equal(refused.bytes.toString(), '', 'oversized request must not report a successful tunnel');
  report.cases.push({ name: 'oversized-complete-header', result: 'passed', transport_errors: refused.errors });
  const incomplete = header.subarray(undefined, header.length - Buffer.byteLength('\r\n'));
  await transmit(running.address, [incomplete]);
  assert.equal(upstream.connections.length, count, 'EOF before the terminator must not connect upstream');
  report.cases.push({ name: 'incomplete-header', result: 'passed' });
  const tunnel = await transmit(running.address, [Buffer.concat([connect, payload])]);
  assert.deepEqual(tunnel.errors, []);
  assert.ok(tunnel.bytes.toString().startsWith('HTTP/1.1 200 Connection Established\r\n\r\n'));
  assert.deepEqual(upstream.connections.map(record => record.bytes), [Buffer.concat([forwarded, payload]), payload],
    'CONNECT bytes read with the header must not disappear');
  assert.ok(upstream.connections.every(record => record.peer === sourceAddress.address),
    'every upstream socket must use the declared interface');
  report.cases.push({ name: 'connect-with-coalesced-payload', result: 'passed' });
  report.result = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  await running?.stop();
  if (upstream) {
    report.connections = upstream.connections;
    await upstream.close();
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
