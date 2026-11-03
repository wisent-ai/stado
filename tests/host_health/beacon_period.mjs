import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the host-health beacon's period on a running host.
// STADO_BEACON_TEST_HOST runs `stado serve --health-interval-seconds P`; the
// journey reads `stado host beacon list --json` for STADO_BEACON_TEST_OBSERVE
// seconds and requires every observation of that host's beacon to be younger
// than 3 * P, and its `reported_at` to advance at least twice. Before the
// repair a loaded Mac's beacon stood still for an hour: the serve loop read
// each unit's launchd events before it published. It also requires the
// published units to carry an `events` status, either the age of the last
// background read or `not read yet`. Read only. Required: STADO_BIN,
// STADO_BEACON_TEST_HOST, STADO_BEACON_TEST_PERIOD (P, seconds),
// STADO_BEACON_TEST_OBSERVE (seconds, at least 3 * P).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const host = required('STADO_BEACON_TEST_HOST');
const period = Number(required('STADO_BEACON_TEST_PERIOD'));
const observe = Number(required('STADO_BEACON_TEST_OBSERVE'));
assert.ok(period > 0 && observe >= 3 * period, 'STADO_BEACON_TEST_OBSERVE must be at least three periods');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'beacon-period-'));
const report = { started_at: new Date().toISOString(), host, period, observe, observations: [], result: 'failed' };
report.stado_sha256 = createHash('sha256').update(readFileSync(stado)).digest('hex');
const save = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));

function observation() {
  const answer = spawnSync(stado, ['host', 'beacon', 'list', '--json'], { encoding: 'utf8', timeout: 120_000 });
  assert.equal(answer.status, 0, (answer.stderr || '').slice(-1000));
  const listed = JSON.parse(answer.stdout);
  const rows = Array.isArray(listed) ? listed : listed.beacons ?? listed.hosts ?? [];
  const row = rows.find(each => (each.host ?? each.target ?? each.name) === host);
  assert.ok(row, `the beacon list names no ${host}: ${answer.stdout.slice(0, 500)}`);
  return row;
}
const reportedAt = row => row.reported_at ?? row.beacon?.reported_at ?? row.published_at;
const ageSeconds = row => (Date.now() - Date.parse(reportedAt(row))) / 1000;

try {
  const until = Date.now() + observe * 1000;
  const seen = new Set();
  while (Date.now() < until) {
    const row = observation();
    const age = ageSeconds(row);
    report.observations.push({ at: new Date().toISOString(), reported_at: reportedAt(row), age });
    save();
    assert.ok(age < 3 * period, `${host}'s beacon is ${age}s old, past three ${period}s periods`);
    seen.add(reportedAt(row));
    await new Promise(next => setTimeout(next, Math.max(1, Math.floor(period / 2)) * 1000));
  }
  assert.ok(seen.size >= 3, `${host}'s beacon advanced ${seen.size - 1} time(s) in ${observe}s`);
  const health = spawnSync(stado, ['host', 'health', host, '--json'], { encoding: 'utf8', timeout: 120_000 });
  assert.equal(health.status, 0, (health.stderr || '').slice(-1000));
  const units = JSON.parse(health.stdout).beacon?.units ?? JSON.parse(health.stdout).units ?? {};
  const statuses = Object.values(units).map(unit => unit.events).filter(Boolean);
  report.event_statuses = statuses.slice(0, 20);
  assert.ok(statuses.length > 0, 'no published unit carries an events status');
  assert.ok(statuses.every(status => /read beside the beacon|not read yet/.test(status)),
    `an events status says neither its age nor that it is unread: ${statuses.join(' | ')}`);
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  save();
  console.log(`report: ${join(output, 'report.json')} (${report.result})`);
}
