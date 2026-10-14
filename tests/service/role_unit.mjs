import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// A unit that runs one of the host Stado process's roles under a label of
// its own is retired by `stado service ensure stado` once the one process is
// proven to run that role. Run on a dedicated macOS service test host whose
// isolated com.wisent.stado runs `serve` with the role named by
// STADO_SERVICE_TEST_ROLE, one that holds no listener and takes no value
// (--worker or --control-plane). The registry and unit belong to an
// isolated home under this checkout's ignored build directory.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
function fixture(variable) {
  const value = process.env[variable];
  assert.ok(value && isAbsolute(value), `${variable} must be an absolute path`);
  const path = realpathSync(value);
  const inside = relative(build, path);
  assert.ok(inside && !inside.startsWith('..'), `${variable} must live under ${build}`);
  return path;
}
const home = fixture('STADO_SERVICE_TEST_HOME');
const config = fixture('STADO_SERVICE_TEST_CONFIG');
const host = process.env.STADO_SERVICE_TEST_HOST;
const executable = process.env.STADO_BIN;
const role = process.env.STADO_SERVICE_TEST_ROLE;
assert.ok(host && executable && role,
  'STADO_SERVICE_TEST_HOST, STADO_BIN and STADO_SERVICE_TEST_ROLE are required');
assert.ok(['--worker', '--control-plane'].includes(role),
  `STADO_SERVICE_TEST_ROLE names a role without a listener or value; got ${role}`);
assert.equal(process.platform, 'darwin', 'This journey drives launchd');
const binary = realpathSync(executable);

const output = mkdtempSync(join(build, 'service-role-unit-'));
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const domain = `gui/${process.getuid()}`;
const roleUnit = `com.wisent.stado.test.role-${randomUUID()}`;
const report = { commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(answer.status !== null && !answer.status,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function plist(label, args) {
  const path = join(output, `${label}.plist`);
  const strings = [binary, ...args].map(word => `<string>${word}</string>`).join('');
  writeFileSync(path, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>${label}</string>
<key>ProgramArguments</key><array>${strings}</array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><false/>
</dict></plist>
`);
  return path;
}
let failure;
const loaded = [];
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const version = success(run(binary, ['--version']));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version };
  const revisionMatch = version.match(/\(rev ([^)]+)\)/);
  assert.ok(revisionMatch, `The binary has no source revision: ${version}`);
  const [, binaryRevision] = revisionMatch;
  assert.equal(binaryRevision, report.source_revision,
    'The tested binary must be built from the recorded committed source');
  const current = JSON.parse(success(run(binary, ['service', 'unit', 'show',
    'com.wisent.stado', '--host', host, '--json'])));
  assert.ok(current.pid, 'The isolated com.wisent.stado must already be running');
  const roles = success(run(binary, ['service', 'roles', '--pid', String(current.pid)]));
  const [served] = roles.split('\n').filter(line => line.startsWith('STADO_SERVE_ROLES\t'));
  assert.ok(served && served.split(/\s+/).includes(role),
    `The one process must run ${role}; it answered: ${roles}`);
  report.before = { current, roles };

  success(run('launchctl', ['bootstrap', domain, plist(roleUnit, ['serve', role])]));
  loaded.push(roleUnit);
  success(run('launchctl', ['print', `${domain}/${roleUnit}`]));

  const ensure = run(binary, ['service', 'ensure', 'stado', '--host', host,
    '--reason', 'the one process runs this role', '--json']);
  const receipt = JSON.parse(success(ensure));
  const retired = receipt.predecessors.find(unit => unit.unit === roleUnit);
  assert.ok(retired, `JSON omitted the role unit ${roleUnit}: ${JSON.stringify(receipt.predecessors)}`);
  assert.equal(retired.state, 'retired', JSON.stringify(retired));
  assert.match(ensure.stderr, new RegExp(`stado replaced ${roleUnit}: retired`),
    'Ensure must name the role unit it retired');
  const gone = run('launchctl', ['print', `${domain}/${roleUnit}`]);
  assert.ok(gone.status, 'The role unit still runs or remains loaded');
  report.after = { receipt, role_unit_absent: true };
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  for (const label of loaded) {
    run('launchctl', ['bootout', `${domain}/${label}`]);
    run('launchctl', ['enable', `${domain}/${label}`]);
  }
  report.scope = 'Real launchd retirement of a unit running a role the one Stado process proves it runs';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
