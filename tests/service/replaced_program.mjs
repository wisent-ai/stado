import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  constants, copyFileSync, mkdtempSync, readFileSync, realpathSync, renameSync, statSync, writeFileSync,
} from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// A unit whose program an install replaced in place (a new file renamed over
// the path, so the path and the process's command name are unchanged while
// the process keeps the old inode) is reloaded by `stado service ensure`,
// never answered `already_correct`. Run on a dedicated macOS service test
// host whose isolated com.wisent.stado runs a program that lives in the test
// home; the registry and unit belong to that home under this checkout's
// ignored build directory.
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
const managed = registry.targets.find(entry => entry.name === host)?.services?.find(entry =>
  entry.label === 'com.wisent.stado' || entry.unit === 'com.wisent.stado');
assert.ok(managed?.program, `${host} must declare an isolated com.wisent.stado with its program`);
const output = mkdtempSync(join(build, 'service-replaced-program-'));
const environment = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
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
const ensure = () => JSON.parse(success(run(binary, ['service', 'ensure', 'stado', '--host', host,
  '--reason', 'converge the unit on its installed program', '--json'])));
// The inode of the image process `pid` executes, as lsof names it.
function runningInode(pid) {
  const listed = success(run('lsof', ['-a', '-p', String(pid), '-d', 'txt', '-Fi']));
  const [inode] = listed.split('\n').filter(line => line.startsWith('i')).map(line => line.slice('i'.length));
  assert.ok(inode, `lsof named no image inode for pid ${pid}: ${listed}`);
  return inode;
}

let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)) };
  const settled = ensure();
  assert.ok(settled.pid, `the isolated unit is not running: ${JSON.stringify(settled)}`);
  const program = realpathSync(managed.program);
  const inside = relative(home, program);
  assert.ok(inside && !inside.startsWith('..'), `the unit's program ${program} is not in the test home`);
  assert.equal(runningInode(settled.pid), String(statSync(program).ino),
    'before the replacement the unit already runs another image');
  report.before = settled;

  // Replace the program in place, as an install does: a copy renamed over the
  // path. Same bytes, same path, a new inode.
  const staged = join(dirname(program), `.replaced-${process.pid}`);
  copyFileSync(program, staged, constants.COPYFILE_EXCL);
  renameSync(staged, program);
  const installedInode = String(statSync(program).ino);
  assert.notEqual(runningInode(settled.pid), installedInode, 'the replacement did not change the inode');

  const replaced = ensure();
  report.after = replaced;
  assert.notEqual(replaced.action, 'already_correct',
    'ensure answered already_correct for a unit still running the replaced image');
  assert.ok(replaced.pid, `ensure left no running process: ${JSON.stringify(replaced)}`);
  assert.equal(runningInode(replaced.pid), installedInode,
    'after ensure the unit does not run the installed program');

  const again = ensure();
  assert.equal(again.action, 'already_correct', 'a unit running its installed program was touched again');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.scope = 'Real launchd unit whose program was replaced in place, reloaded by ensure';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
