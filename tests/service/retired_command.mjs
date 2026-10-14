import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Run on a dedicated macOS service test host. The registry and managed Stado
// unit must belong to an isolated home under this checkout's ignored build
// directory; an operator's installed service is never an acceptable fixture.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
function fixture(variable) {
  const value = process.env[variable];
  assert.ok(value, `${variable} must name a dedicated test fixture`);
  const path = realpathSync(value);
  const within = relative(build, path);
  assert.ok(within && !isAbsolute(within) && !within.startsWith('..'),
    `${variable} must be inside ${build}`);
  return path;
}
const home = fixture('STADO_SERVICE_TEST_HOME');
const config = fixture('STADO_SERVICE_TEST_CONFIG');
const host = process.env.STADO_SERVICE_TEST_HOST;
const executable = process.env.STADO_BIN;
assert.ok(host && executable, 'STADO_SERVICE_TEST_HOST and STADO_BIN are required');
assert.equal(process.platform, 'darwin', 'This journey drives launchd');
const binary = realpathSync(executable);
const registry = JSON.parse(readFileSync(join(home, '.stado/local-storage/registry.json')));
const target = registry.targets.find(entry => entry.name === host);
const managed = target?.services?.find(entry =>
  entry.label === 'com.wisent.stado' || entry.unit === 'com.wisent.stado');
assert.ok(managed, `${host} must declare an isolated com.wisent.stado`);
assert.ok(managed.path, 'The managed unit needs a unit file');
const unitFile = realpathSync(managed.path);
assert.ok(relative(home, unitFile) && !relative(home, unitFile).startsWith('..'),
  'The managed unit file must belong to the dedicated test home');

const output = mkdtempSync(join(build, 'service-retired-command-'));
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const domain = `gui/${process.getuid()}`;
const retired = `com.wisent.stado.test.retired-${randomUUID()}`;
const kept = `com.wisent.stado.test.kept-${randomUUID()}`;
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
function plist(label, argument) {
  const path = join(output, `${label}.plist`);
  writeFileSync(path, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>${label}</string>
<key>ProgramArguments</key><array><string>${binary}</string><string>${argument}</string></array>
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
  assert.ok(current.pid, 'The isolated replacement must already be running');
  report.before = { current };

  for (const [label, verb] of [[retired, 'disk-cleanup'], [kept, '--version']]) {
    success(run('launchctl', ['bootstrap', domain, plist(label, verb)]));
    loaded.push(label);
    success(run('launchctl', ['print', `${domain}/${label}`]));
  }
  const retiredVerb = run(binary, ['disk-cleanup', '--help']);
  assert.ok(retiredVerb.status, 'The old root command must be refused');
  const ensure = run(binary, ['service', 'ensure', 'stado', '--host', host,
    '--reason', 'replace an obsolete recurring command', '--json']);
  const receipt = JSON.parse(success(ensure));
  const retiredResult = receipt.predecessors.find(unit => unit.unit === retired);
  assert.ok(retiredResult, `JSON omitted retirement result for ${retired}`);
  assert.equal(retiredResult.state, 'retired');
  assert.match(retiredResult.detail, /removed root command:/,
    'JSON must carry the cause for Desktop');
  const keptResult = receipt.predecessors.find(unit => unit.unit === kept);
  assert.ok(keptResult, `JSON omitted kept result for ${kept}`);
  assert.equal(keptResult.state, 'kept');
  assert.match(ensure.stderr, new RegExp(`retired ${retired} before starting: retired \\(removed root command:`),
    'Ensure must name the withdrawn unit and the cause');
  const gone = run('launchctl', ['print', `${domain}/${retired}`]);
  assert.ok(gone.status, 'The obsolete unit still runs or remains loaded');
  success(run('launchctl', ['print', `${domain}/${kept}`]));
  assert.match(ensure.stderr, new RegExp(`${kept}: kept`),
    'A valid one-pass unit must not be withdrawn');
  report.after = { receipt, retired_absent: true, valid_unit_present: true };
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  for (const label of loaded) {
    run('launchctl', ['bootout', `${domain}/${label}`]);
    run('launchctl', ['enable', `${domain}/${label}`]);
  }
  report.scope = 'Real launchd predecessor retirement and valid-command preservation on an isolated host';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
