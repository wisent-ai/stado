// Runs the real proxy through its CLI and TCP sockets: what it refuses, and
// that nothing it refuses reaches an upstream. No browser or provider stand-in.
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
const output = mkdtempSync(join(root, '.build', 'egress-refusals-'));
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
  report.version = command(report, binary, ['--version'], options).stdout;
  const addresses = networkInterfaces();
  const sourceAddress = addresses[iface]?.find(address => address.family === 'IPv4' && !address.internal);
  assert.ok(sourceAddress, `${iface} must have a real non-loopback IPv4 address`);
  const loopback = Object.values(addresses).flat().find(address => address.family === 'IPv4' && address.internal);
  assert.ok(loopback, 'a loopback IPv4 address is required');
  upstream = await receiver(sourceAddress.address);
  const destination = `${upstream.address.address}:${upstream.address.port}`;
  const reservation = await receiver(loopback.address);
  const port = String(reservation.address.port);
  await reservation.close();
  const budget = String(Buffer.byteLength(`CONNECT ${destination} HTTP/1.1\r\nHost: ${destination}\r\n\r\n`));

  // The listen address is the deployment's to state: no bind is assumed.
  const unbound = command(report, binary,
    ['egress', 'mobile', 'serve', '--interface', iface, '--port', port, '--max-header-bytes', budget], options);
  assert.ok(unbound.status, 'a serve without --bind must refuse without starting a listener');
  assert.ok(unbound.stderr.includes('--bind'), 'the missing --bind must be named');
  report.cases.push({ name: 'missing-bind', result: 'passed' });

  // A non-loopback bind is refused before anything listens.
  const exposed = command(report, binary,
    ['egress', 'mobile', 'serve', '--interface', iface, '--bind', sourceAddress.address, '--port', port,
      '--max-header-bytes', budget], options);
  assert.ok(exposed.status, 'a non-loopback bind must refuse');
  assert.ok(exposed.stderr.includes('loopback'), 'the refusal must say the proxy listens only on loopback');
  report.cases.push({ name: 'non-loopback-bind', result: 'passed' });

  running = await proxy(report, binary,
    ['egress', 'mobile', 'serve', '--interface', iface, '--bind', loopback.address, '--port', port,
      '--max-header-bytes', budget], options);

  // A request line that is not METHOD TARGET HTTP-VERSION is answered 400 and
  // reaches no upstream.
  const malformed = await transmit(running.address, [Buffer.from(`GET\r\nHost: ${destination}\r\n\r\n`)]);
  assert.ok(malformed.bytes.toString().startsWith('HTTP/1.1 400 Bad Request\r\n'),
    'a malformed request line must be answered 400');
  assert.deepEqual(upstream.connections, [], 'a malformed request must not connect upstream');
  report.cases.push({ name: 'malformed-request-line', result: 'passed' });

  // A CONNECT target names its port; one without it is refused, never sent
  // to an assumed port.
  const portless = await transmit(running.address,
    [Buffer.from(`CONNECT ${upstream.address.address} HTTP/1.1\r\nHost: ${upstream.address.address}\r\n\r\n`)]);
  assert.ok(!portless.bytes.toString().startsWith('HTTP/1.1 200'), 'a port-less CONNECT must not open a tunnel');
  assert.deepEqual(upstream.connections, [], 'a port-less CONNECT must not connect upstream');
  report.cases.push({ name: 'connect-without-port', result: 'passed', transport_errors: portless.errors });
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
