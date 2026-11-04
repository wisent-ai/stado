// Real journey: `stado serve` comes up while its Tailscale client never answers.
//
// A Tailscale client that does not answer `status --json` (the macOS app
// binary started from a launchd agent sits in its event loop) held serve's
// start in `host identity` for as long as it ran: no API, no resolver, no role
// after them. The tailnet map is now read beside the roles.
//
// `tailnet.clients` names one program of this run's in every deployment: a
// client that records its pid and never exits. Two isolated deployments
// under .build/serve-provider-client, each with its own HOME and config:
//
// A. A local store holding the shared directory template, this machine as
//    `marker-host` and the directory's authority; the real built Stado runs
//    `serve --api --resolver --health-interval-seconds 1`.
//    1. The API answers /healthz and the resolver its /health while the
//       client still runs.
//    2. The beacon the health role stores names the client `no_answer`, with
//       its pid and since when.
//    3. The next beacon names the same pid, and the client was started once:
//       a pass while a read runs starts none.
//    4. SIGTERM to the serve pid alone: serve ends, and the client it started
//       is gone with it and no open wait of the register names it.
//    5. Restarted with `tailnet.clients` naming a path that holds nothing, the
//       beacon says `absent`, and serve comes up the same way.
// B. The shape that hung: a fleet store behind plain HTTP to an address that
//    is not loopback, which only the tailnet map can admit, so building its
//    client asks the map; the registry is this host's last-known-good copy
//    (`registry push --last-known-good`, the authority another target).
//    `serve --resolver` binds and its /health answers while the client runs,
//    and the client ends with serve.
// The journey ends every process it started; the clients' own ends are
// asserted before it does.
//
// Needs STADO_BIN (the built stado), STADO_TEST_SOURCE_REVISION (the commit it
// was built from) and STADO_TEST_REQUEST_LIMITS (the API byte bounds a
// qualification run declares).
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { hostname as systemHostname } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value, `${name} is required; a missing real fixture is not a passing run`);
  return value;
}
const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const output = join(repository, '.build', 'serve-provider-client', randomUUID());
mkdirSync(output, { recursive: true });
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = { started_at: new Date().toISOString(), commands: [], serves: [], verdict: 'failed' };
const save = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));
const running = [];
// An address reserved for documentation (RFC 5737): never loopback, never a
// tailnet node, so plain HTTP to it is admitted only by the tailnet map.
const UNREACHABLE_STORE = 'http://192.0.2.1:9';
// The line the health role prints after each stored beacon: the host slug.
const SLUG = /^[a-z0-9][a-z0-9-]*$/;

let subject;

function deployment(name, storage) {
  const home = join(output, name);
  mkdirSync(join(home, 'tmp'), { recursive: true });
  const environment = { PATH: required('PATH'), HOME: home, TMPDIR: join(home, 'tmp'),
    STADO_CONFIG: join(home, '.stado', 'config.json'), ...storage };
  const stado = args => {
    const result = spawnSync(subject.path, args, { cwd: repository, env: environment, encoding: 'utf8' });
    report.commands.push({ deployment: name, args, status: result.status, signal: result.signal,
      stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
    save();
    assert.ok(exitedCleanly(result), `stado ${args.join(' ')}: ${result.stderr}`);
    return result.stdout;
  };
  stado(['config', 'init']);
  stado(['config', 'set', 'dashboard.request_limits', required('STADO_TEST_REQUEST_LIMITS')]);
  return { name, home, environment, stado, freeBind: () => `127.0.0.1:${stado(['host', 'local', 'port']).trim()}` };
}

// A client that writes its pid and then waits on a pipe nobody writes to.
function hangingClient() {
  const pids = join(output, 'client.pids');
  const program = join(output, 'tailscale');
  writeFileSync(program, `#!/bin/sh\necho $$ >> '${pids}'\nexec tail -f /dev/null\n`);
  chmodSync(program, 0o700);
  return { program: realpathSync(program), pids };
}
const clientPids = pids => readFileSync(pids, 'utf8').trim().split('\n').filter(Boolean).map(Number);
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };

// The shared directory template with this machine as `marker-host`, its
// resolver API at `apiBind`; `authority` names the directory's authority.
function registry(at, apiBind, authority) {
  const document = JSON.parse(readFileSync(join(repository, 'tests/service_directory/registry.json'), 'utf8'));
  const [target] = document.targets;
  target.hostnames = [systemHostname().toLowerCase()];
  target.service_resolver.api_bind = apiBind;
  target.service_resolver.adapters = [];
  document.service_directory.authority.target = authority;
  const path = join(at.home, 'registry.json');
  writeFileSync(path, JSON.stringify(document, null, '  '));
  return path;
}

// `stado serve ARGS` in `at`; `ready` resolves once its resolver serves and,
// with `beacons`, once its first beacon is stored.
function serve(at, label, args, beacons) {
  const child = spawn(subject.path, args, { cwd: repository, env: at.environment, stdio: ['ignore', 'pipe', 'pipe'] });
  running.push(child);
  const record = { deployment: at.name, label, args, pid: child.pid, stdout: [], stderr: [] };
  report.serves.push(record);
  let stored = 0;
  let serving = false;
  let ended = null;
  let wake = () => {};
  createInterface({ input: child.stdout }).on('line', line => {
    record.stdout.push(line); save();
    if (SLUG.test(line)) { stored += 1; wake(); }
  });
  createInterface({ input: child.stderr }).on('line', line => {
    record.stderr.push(line); save();
    if (line.startsWith('stado resolver target=')) { serving = true; wake(); }
  });
  child.on('exit', (code, signal) => { ended = { code, signal }; record.exit = ended; save(); wake(); });
  const until = condition => new Promise((done, failed) => {
    const check = () => {
      if (condition()) return done();
      if (ended) return failed(new Error(`serve ${label} ended (${JSON.stringify(ended)}); see ${output}`));
      wake = check;
    };
    check();
  });
  const nextBeacon = () => { const seen = stored; return until(() => stored > seen); };
  return { child, ready: until(() => serving && (!beacons || stored > 0)), nextBeacon };
}

async function answers(url) {
  const response = await fetch(url);
  const body = await response.text();
  report.commands.push({ get: url, status: response.status, body });
  save();
  assert.equal(response.status, 200, `${url} answered ${response.status}: ${body}`);
  return body;
}

function beacon(at, store) {
  const listed = JSON.parse(at.stado(['host', 'beacon', 'list', '--json']));
  const row = listed.hosts.find(row => row.host === 'marker-host');
  assert.ok(row && row.beacon, `beacon list names marker-host's beacon: ${JSON.stringify(listed)}`);
  return JSON.parse(readFileSync(join(store, row.beacon), 'utf8'));
}

async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise(done => child.once('exit', done));
  child.kill('SIGTERM');
  await exited;
}

// SIGTERM to `child` alone, then: the client `pid` it started is gone, and
// its wait was closed. A wait serve left unclosed when it died is listed as
// `ended` (its process no longer runs), so neither list may name the client.
async function endsWithServe(at, child, pid) {
  await stop(child);
  assert.equal(child.signalCode, 'SIGTERM', `serve ended by its signal: ${child.exitCode}/${child.signalCode}`);
  assert.ok(!alive(pid), `the client ${pid} serve started ended with it`);
  const waits = JSON.parse(at.stado(['waits', '--json']));
  const named = [...waits.open, ...waits.ended].filter(wait => wait.line.includes(`pid ${pid} in `));
  assert.deepEqual(named, [], `serve closed the wait on its ended client: ${JSON.stringify(waits)}`);
}

try {
  report.source_revision = required('STADO_TEST_SOURCE_REVISION');
  subject = snapshotSubject(required('STADO_BIN'), output);
  report.subject = subject;
  report.journey_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const client = hangingClient();

  // A. A local store.
  const store = join(output, 'a-store');
  mkdirSync(store, { recursive: true });
  const a = deployment('a', { WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: store });
  verifyRevision(a.stado(['--version']).trim(), report.source_revision);
  a.stado(['config', 'set', 'tailnet.clients', JSON.stringify([client.program])]);
  const resolverBind = a.freeBind();
  a.stado(['registry', 'import', registry(a, resolverBind, 'marker-host')]);
  const apiPort = a.freeBind().split(':')[1];
  const roles = ['serve', '--api', '--bind', '127.0.0.1', '--port', apiPort, '--api-local-store', store,
    '--resolver', '--target', 'marker-host', '--health-interval-seconds', '1'];

  // 1. Both roles answer while the client still runs.
  const first = serve(a, 'hanging-client', roles, true);
  await first.ready;
  const [pid] = clientPids(client.pids);
  assert.ok(pid && alive(pid), `the client this run declared was started and still runs: ${pid}`);
  await answers(`http://127.0.0.1:${apiPort}/healthz`);
  const health = JSON.parse(await answers(`http://${resolverBind}/health`));
  assert.equal(health.status, 'ok', JSON.stringify(health));

  // 2. The beacon names the client as not answering.
  const named = beacon(a, store).tailscale_client;
  report.tailscale_client = named;
  assert.equal(named.state, 'no_answer', JSON.stringify(named));
  assert.equal(named.pid, pid, JSON.stringify(named));
  assert.equal(named.client, client.program, JSON.stringify(named));
  assert.ok(Date.parse(named.since), JSON.stringify(named));

  // 3. The next pass starts no second client.
  await first.nextBeacon();
  assert.equal(beacon(a, store).tailscale_client.pid, pid, 'the next beacon names the same client');
  assert.deepEqual(clientPids(client.pids), [pid], 'one client was started for every pass');
  // 4. SIGTERM to serve alone ends its client.
  await endsWithServe(a, first.child, pid);

  // 5. No client at the declared path: absent, and serve still comes up.
  a.stado(['config', 'set', 'tailnet.clients', JSON.stringify([join(output, 'no-client-here')])]);
  const second = serve(a, 'no-client', roles, true);
  await second.ready;
  await answers(`http://127.0.0.1:${apiPort}/healthz`);
  await second.nextBeacon();
  const absent = beacon(a, store).tailscale_client;
  report.tailscale_client_absent = absent;
  assert.equal(absent.state, 'absent', JSON.stringify(absent));
  await stop(second.child);

  // B. A store only the tailnet map can admit.
  const b = deployment('b', {});
  const bResolver = b.freeBind();
  b.stado(['registry', 'push', registry(b, bResolver, 'other-host'), '--last-known-good']);
  const token = join(b.home, 'store-token');
  writeFileSync(token, `journey-${randomUUID()}`, { mode: 0o600 });
  for (const [key, value] of [['storage.stado.url', UNREACHABLE_STORE], ['storage.stado.token_file', token],
    ['storage.backend', 'stado'], ['tailnet.clients', JSON.stringify([client.program])]]) {
    b.stado(['config', 'set', key, value]);
  }
  const before = clientPids(client.pids).length;
  const third = serve(b, 'tailnet-store', ['serve', '--resolver', '--target', 'marker-host'], false);
  await third.ready;
  const bHealth = JSON.parse(await answers(`http://${bResolver}/health`));
  assert.equal(bHealth.status, 'ok', JSON.stringify(bHealth));
  const started = clientPids(client.pids).slice(before);
  assert.equal(started.length, 1, `serve started its client once: ${started}`);
  assert.ok(alive(started[0]), 'the resolver answered while the client still ran');
  report.store_client_leg = { client_pid: started[0], resolver: bHealth };
  await endsWithServe(b, third.child, started[0]);

  report.verdict = 'passed';
} finally {
  for (const child of running) await stop(child);
  const pids = join(output, 'client.pids');
  if (existsSync(pids)) for (const pid of clientPids(pids)) if (alive(pid)) process.kill(pid, 'SIGTERM');
  report.ended_at = new Date().toISOString();
  save();
  console.log(`serve provider-client evidence: ${output} (${report.verdict})`);
}
