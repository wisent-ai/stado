import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Qualification of `stado placement standby` against the fleet registry: this
// machine stands by for a placement profile placed on another host. The pass
// must end `declared` with every unit of the profile staged on this machine:
// the unit file present at the path the receipt names, nothing loaded under
// its label, the profile's template for this host naming exactly those units,
// no unit declared on this host, and the service directory naming this host
// either as an endpoint or as a standby, never both. A second pass stages the
// same units again. Refusals: an empty reason, and the placed host standing by
// for its own profile. Required: STADO_BIN, the Stado under test;
// STADO_PLACEMENT_TEST_PROFILE, the profile; STADO_PLACEMENT_TEST_HOST, this
// machine's registry name; STADO_PLACEMENT_TEST_REASON, why it stands by.
// macOS only: the units are read back from launchd.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const profile = required('STADO_PLACEMENT_TEST_PROFILE');
const host = required('STADO_PLACEMENT_TEST_HOST');
const reason = required('STADO_PLACEMENT_TEST_REASON');
assert.equal(process.platform, 'darwin', 'This journey reads the staged units back from launchd');
const output = mkdtempSync(join(build, 'placement-standby-'));
const domain = `gui/${process.getuid()}`;
const report = { started_at: new Date().toISOString(), profile, host, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const succeeded = answer => answer.status !== null && !answer.status;
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function refused(answer, said) {
  assert.ok(answer.status !== null && answer.status, `expected a refusal, got ${answer.status}: ${answer.stdout}`);
  assert.match(answer.stderr, said);
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const standby = (on, why) => run(binary, ['placement', 'standby', profile, '--host', on, '--reason', why, '--json']);

  refused(standby(host, ' '), /--reason must say why/);

  const first = JSON.parse(success(standby(host, reason)));
  report.first_receipt = first;
  assert.equal(first.outcome, 'declared', first.detail);
  assert.equal(first.reason, reason);
  assert.ok(first.units.length, 'the profile stages at least one unit');
  for (const unit of first.units) {
    assert.equal(unit.action, 'staged', JSON.stringify(unit));
    assert.ok(existsSync(unit.path), `${unit.label} is staged at ${unit.path}`);
    const loaded = run('launchctl', ['print', `${domain}/${unit.label}`]);
    assert.ok(!succeeded(loaded), `${unit.label} must not be loaded on a standby host`);
  }

  const registry = JSON.parse(success(run(binary, ['registry', 'pull'])));
  const placement = registry.placement_profiles.find(entry => entry.name === profile);
  assert.ok(placement, `the registry keeps profile ${profile}`);
  const template = placement.hosts[host];
  assert.ok(template, `${profile} names ${host}`);
  for (const unit of first.units) {
    assert.deepEqual(
      { name: template.units[unit.service].name, path: template.units[unit.service].path },
      { name: unit.label, path: unit.path },
    );
    const route = registry.service_directory.services[unit.service];
    if (route) {
      assert.notEqual(Boolean(route.endpoints?.[host]), Boolean(route.standby?.[host]),
        `${unit.service} names ${host} as an endpoint or as a standby, never both or neither`);
    }
  }
  const declared = registry.targets.find(target => target.name === host).services
    .filter(service => first.units.some(unit => unit.label === service.name || unit.service === service.name));
  assert.deepEqual(declared, [], 'a standby host declares none of the units it stages');

  const second = JSON.parse(success(standby(host, reason)));
  report.second_receipt = second;
  assert.equal(second.outcome, 'declared', second.detail);
  assert.deepEqual(second.units, first.units, 'a second pass stages the same units');

  refused(standby(first.placed_on, reason), /cannot stand by for its own profile/);
  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = String(error.stack || error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(join(output, 'report.json'));
}
if (failure) throw failure;
