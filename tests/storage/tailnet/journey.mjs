// Real Tailscale membership and real Stado object services, never a status stub.
// Each case supplies an isolated, already-serving object API on this host's
// reported tailnet address, its owner-only bearer file and its local store.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs';
import { isIPv4, isIPv6 } from 'node:net';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../../native/subject.mjs';

class PrerequisiteError extends Error {}
function prerequisite(condition, cause) {
  if (!condition) throw new PrerequisiteError(cause);
}
function required(name) {
  const value = process.env[name];
  prerequisite(value, `${name} is required; a missing real fixture is not a passing run`);
  return value;
}
const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const build = realpathSync(join(repository, '.build'));
const output = join(build, 'storage-tailnet', randomUUID());
mkdirSync(output, { recursive: true });
const report = { source_revision: process.env.STADO_TEST_SOURCE_REVISION,
  started_at: new Date().toISOString(), commands: [], cases: [], verdict: 'failed' };
const save = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function insideBuild(path) {
  const actual = realpathSync(path);
  const suffix = relative(build, actual);
  assert.ok(suffix && !isAbsolute(suffix) && suffix !== '..' && !suffix.startsWith(`..${sep}`),
    `${actual} must be inside this checkout's ignored .build directory`);
  return actual;
}
function hostname(origin) {
  return new URL(origin).hostname.replaceAll('[', '').replaceAll(']', '');
}
function invoke(program, args, environment) {
  const result = spawnSync(program, args, { cwd: repository, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  save();
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'a killed dependency is not a useful product refusal');
  return result;
}
function queued(store, namespace) {
  const root = join(store, 'ecosystem', namespace, 'queue');
  if (!existsSync(root)) return new Map();
  return new Map(readdirSync(root).filter(name => name.endsWith('.json'))
    .map(name => [name, readFileSync(join(root, name))]));
}
try {
  report.source_revision = required('STADO_TEST_SOURCE_REVISION');
  const subject = snapshotSubject(required('STADO_BIN'), output);
  report.subject = subject;
  report.journey_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const tailscale = realpathSync(required('TAILSCALE'));
  report.tailscale_binary_sha256 = digest(readFileSync(tailscale));
  const status = invoke(tailscale, ['status', '--json'], process.env);
  prerequisite(exitedCleanly(status), `the real Tailscale node did not answer: ${status.stderr}`);
  const observed = JSON.parse(status.stdout);
  const self = observed.Self;
  prerequisite(self && Array.isArray(self.TailscaleIPs), 'the real Tailscale node must report its own addresses');
  const members = new Set(self.TailscaleIPs);
  const reported = new Set(members);
  if (observed.Peer !== undefined && observed.Peer !== null) {
    prerequisite(typeof observed.Peer === 'object' && !Array.isArray(observed.Peer),
      'the real Tailscale status did not report peers as an object');
    for (const peer of Object.values(observed.Peer)) {
      prerequisite(Array.isArray(peer.TailscaleIPs), 'a real Tailscale peer did not report its addresses');
      for (const address of peer.TailscaleIPs) reported.add(address);
    }
  }
  const cases = JSON.parse(required('STADO_TEST_TAILNET_CASES'));
  prerequisite(Array.isArray(cases), 'STADO_TEST_TAILNET_CASES must describe real isolated services');
  prerequisite(cases.some(value => isIPv4(hostname(value.origin))), 'a real reported IPv4 service is required');
  prerequisite(cases.some(value => isIPv6(hostname(value.origin))), 'a real reported IPv6 service is required');
  for (const fixture of cases) {
    const origin = new URL(fixture.origin);
    assert.equal(origin.protocol, 'http:', 'this journey exercises the reported-tailnet plain-HTTP exception');
    assert.ok(members.has(hostname(fixture.origin)), `${fixture.origin} is not assigned to the real local Tailscale node`);
    const store = insideBuild(fixture.store);
    const token = insideBuild(fixture.token_file);
    assert.ok(fixture.namespace && !fixture.namespace.includes('/') && fixture.namespace !== '..',
      'each fixture must name one isolated namespace');
    const home = join(output, randomUUID());
    mkdirSync(join(home, 'scratch'), { recursive: true });
    const environment = { PATH: required('PATH'), HOME: home, TMPDIR: join(home, 'scratch'),
      STADO_CONFIG: join(home, '.stado', 'config.json') };
    const command = args => {
      const result = invoke(subject.path, args, environment);
      assert.ok(exitedCleanly(result), result.stderr);
      return result;
    };
    const version = command(['--version']).stdout.trim();
    verifyRevision(version, report.source_revision);
    command(['config', 'init']);
    for (const [key, value] of [
      ['storage.stado.url', fixture.origin],
      ['storage.stado.namespace', fixture.namespace],
      ['storage.stado.token_file', token],
      ['storage.backend', 'stado'],
    ]) command(['config', 'set', key, value]);
    const before = queued(store, fixture.namespace);
    const run = `tailnet-transport-${randomUUID()}`;
    command(['submit', '--run-id', run, 'true']);
    const after = queued(store, fixture.namespace);
    const [written, ...others] = [...after].filter(([name]) => !before.has(name));
    assert.ok(written, 'the real tailnet object API did not persist the submitted job');
    assert.deepEqual(others, [], 'one submission must create one queue entry');
    const [name, body] = written;
    const job = JSON.parse(body);
    const statuses = JSON.parse(command(['status', '--json']).stdout);
    assert.deepEqual(statuses.find(value => value.job_id === job.job_id),
      { ...job, state: 'queue', reaped: false }, 'the client must read the exact persisted job back over the tailnet');
    for (const [existing, bytes] of before) assert.deepEqual(after.get(existing), bytes,
      'transport qualification must not replace a pre-existing job');
    const refusedOrigin = required('STADO_TEST_TAILNET_REFUSED_ORIGIN');
    const refusedHost = hostname(refusedOrigin);
    assert.equal(new URL(refusedOrigin).protocol, 'http:');
    assert.ok(isIPv4(refusedHost) || isIPv6(refusedHost), 'the refused origin must be an IP literal');
    prerequisite(!reported.has(refusedHost), 'the refused origin must not belong to any node the real status reports');
    command(['config', 'set', 'storage.stado.url', refusedOrigin]);
    const refused = invoke(subject.path, ['status', '--json'], environment);
    assert.ok(!exitedCleanly(refused), 'plain HTTP to an unreported non-loopback address was accepted');
    assert.ok(refused.stderr.includes('must be an HTTPS origin') && refused.stderr.includes(refusedHost),
      `the refusal must identify the transport boundary, not an unrelated failure: ${refused.stderr}`);
    assert.deepEqual(queued(store, fixture.namespace), after, 'a refused origin changed persisted queue state');
    report.cases.push({ origin: fixture.origin, namespace: fixture.namespace, store,
      queue_object: name, sha256: digest(body), job_id: job.job_id });
    save();
  }
  report.verdict = 'passed';
} catch (error) {
  if (error instanceof PrerequisiteError) report.verdict = 'blocked';
  report.error = error.stack;
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  save();
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
