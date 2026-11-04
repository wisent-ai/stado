import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  constants, mkdtempSync, readdirSync, readFileSync, realpathSync, writeFileSync,
} from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { pinnedGit } from '../native/home.mjs';
import { fileURLToPath } from 'node:url';

// A `stado serve --resolver` unit whose process stands in a start step past
// that step's due moment is restarted by `stado service ensure`, never
// answered `already_correct`: launchd calls such a unit running while it binds
// no API and no resolver, and an install of Stado waited on it for hours.
// Run on a dedicated macOS service test host whose isolated com.wisent.stado
// reads its registry through the resolver; the registry, unit and wait
// register belong to the test home under this checkout's ignored build
// directory. STADO_SERVICE_TEST_STUCK names a command that, run in the test
// home, makes that unit's next start hang at a step (the host's own way to
// withhold an answer the step waits on: a Tailscale client that never
// answers, a registry authority that accepts and never replies); the journey
// restarts the unit onto it, waits for the step to pass its due moment in the
// unit's own wait register, undoes the hang with STADO_SERVICE_TEST_UNSTUCK,
// and requires one ensure to restart the unit onto a process that serves.
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
const hang = process.env.STADO_SERVICE_TEST_STUCK;
const unhang = process.env.STADO_SERVICE_TEST_UNSTUCK;
assert.ok(host && executable, 'STADO_SERVICE_TEST_HOST and STADO_BIN are required');
assert.ok(hang && unhang, 'STADO_SERVICE_TEST_STUCK and STADO_SERVICE_TEST_UNSTUCK are required');
assert.equal(process.platform, 'darwin', 'This journey drives launchd');
const binary = realpathSync(executable);
const output = mkdtempSync(join(build, 'service-stuck-start-'));
const environment = { ...pinnedGit, PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: home, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(answer.status !== null && !answer.status,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const ensure = () => run(binary, ['service', 'ensure', 'stado', '--host', host,
  '--reason', 'a unit stuck at start is restarted', '--json']);
// The `stado serve start:` wait process `pid` stands in, with its due moment
// and whether a wait below it is past its own due moment, from the unit's
// own register: a start is stuck only when what it waits on is overdue too.
function startWait(pid) {
  const waits = join(home, '.stado/waits');
  const lines = readdirSync(waits).filter(entry => entry.startsWith(`${pid}-`))
    .map(name => ({ sequence: Number(name.split('-')[1]), line: readFileSync(join(waits, name), 'utf8').trim() }))
    .sort((a, b) => a.sequence - b.sequence);
  const field = (line, name) => line.split('; ').find(part => part.startsWith(`${name}: `))?.slice(name.length + 2);
  const start = lines.find(({ line }) => line.replace(/^czekam: /, '').startsWith('stado serve start: '));
  if (!start) return undefined;
  const due = field(start.line, 'termin');
  const overdue = lines.filter(({ sequence, line }) => sequence > start.sequence
    && Date.parse(field(line, 'termin') ?? '') < Date.now());
  return { what: start.line.replace(/^czekam: /, '').split('; ')[0], due, overdue: overdue.length > 0 };
}
// The register publishes no change when a due moment passes, so the unit's
// status is read again a second later.
function changed() {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1000);
}

let failure;
try {
  report.source_revision = success(run('git', ['-C', root, 'rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)) };
  const settled = JSON.parse(success(ensure()));
  assert.ok(settled.pid, `the isolated unit is not running: ${JSON.stringify(settled)}`);
  report.before = settled;

  success(run('/bin/sh', ['-c', hang]));
  success(run(binary, ['service', 'restart', 'stado', '--host', host, '--json']));
  let stuck;
  for (;;) {
    const listed = JSON.parse(success(run(binary, ['service', 'status', 'stado', '--host', host, '--json'])));
    const pid = [listed].flat().map(entry => entry.pid).find(Boolean);
    const wait = pid && startWait(pid);
    if (wait?.due && Date.parse(wait.due) < Date.now() && wait.overdue) {
      stuck = { pid, ...wait };
      break;
    }
    changed();
  }
  report.stuck = stuck;
  success(run('/bin/sh', ['-c', unhang]));

  const repaired = ensure();
  report.repair = { exit_status: repaired.status, stderr: repaired.stderr };
  const answered = JSON.parse(success(repaired));
  assert.ok(repaired.stderr.includes(stuck.what),
    `ensure did not name the step the unit stood in: ${repaired.stderr}`);
  assert.notEqual(String(answered.pid), String(stuck.pid),
    'ensure left the stuck process running the unit');
  const again = JSON.parse(success(ensure()));
  assert.equal(again.action, 'already_correct', 'a unit that serves was touched again');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.scope = 'Real launchd unit stuck at a start step, restarted by ensure';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
