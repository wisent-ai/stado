import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// The janitor's Docker cleaner against a real Docker daemon on a dedicated
// test host whose disk is the daemon's: a stopped container and an image no
// container uses are pruned at the disk-full rule, a running container and its
// image are kept, and `stado space reclaim --local --apply` reports the
// cleaner. A host whose daemon root is on another volume keeps everything and
// says so. The test makes its own containers, labelled with this run, and
// removes whatever is left after.
// STADO_BIN selects the Stado under test; STADO_DOCKER_TEST_IMAGE names a
// small image the host can pull (the operator's choice, nothing assumed). The
// host's disk must be at the rule for the pass to enforce; the test fills it
// with a file under its own build directory and removes the file after.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'space-docker-reclaim-'));
const stado = process.env.STADO_BIN;
const image = process.env.STADO_DOCKER_TEST_IMAGE;
const label = `stado.test.run=${process.pid}`;
const report = { started_at: new Date().toISOString(), image, label, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(exitedCleanly(answer), JSON.stringify({ stderr: answer.stderr, error: answer.error?.message,
    signal: answer.signal, status: answer.status }));
  return answer.stdout.trim();
}
const exists = (kind, name) => exitedCleanly(run('docker', [kind, 'inspect', name]));
let fill;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado || !image || !exitedCleanly(run('docker', ['info']))) {
    report.result = 'blocked';
    throw new Error(`requires STADO_BIN, STADO_DOCKER_TEST_IMAGE and a Docker daemon this user reaches; missing: ${[
      !stado && 'STADO_BIN', !image && 'STADO_DOCKER_TEST_IMAGE',
    ].filter(Boolean).join(', ') || 'the daemon'}`);
  }
  report.stado = snapshotSubject(stado, output);
  const binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);

  success(run('docker', ['pull', image]));
  const kept = `stado-test-running-${process.pid}`;
  const stopped = `stado-test-stopped-${process.pid}`;
  success(run('docker', ['run', '--detach', '--label', label, '--name', kept, image, 'sleep', '600']));
  success(run('docker', ['create', '--label', label, '--name', stopped, image, 'true']));
  const unused = `stado-test-unused:${process.pid}`;
  success(run('docker', ['commit', stopped, unused]));

  // Bring the volume the rule judges to its threshold with a file of our own.
  const disk = JSON.parse(success(run(binary, ['space', 'candidates', '--json'])));
  const headroom = disk.headroom_bytes;
  assert.ok(Number.isSafeInteger(headroom), `space candidates states no headroom: ${JSON.stringify(disk)}`);
  fill = join(output, 'fill.bin');
  if (headroom > 0) success(run('fallocate', ['-l', String(headroom + 1024 * 1024 * 1024), fill]));

  const pass = JSON.parse(success(run(binary, ['space', 'reclaim', '--local', '--apply', '--reason', 'docker reclaim test'])));
  report.pass = pass;
  const cleaner = pass.cleaners?.docker;
  assert.ok(cleaner, `the pass reports no docker cleaner: ${JSON.stringify(pass.cleaners)}`);
  if (cleaner.skipped?.docker_root_not_on_judged_volume) {
    assert.ok(exists('container', stopped), 'a daemon on another volume was pruned');
    report.result = 'passed';
    report.scope_note = 'the daemon root is on another volume: kept and said so';
  } else {
    assert.ok(cleaner.deleted_items > 0, `the docker cleaner removed nothing: ${JSON.stringify(cleaner)}`);
    assert.ok(!exists('container', stopped), 'the stopped container survived the rule');
    assert.ok(!exists('image', unused), 'the unused image survived the rule');
    assert.ok(exists('container', kept), 'the running container was removed');
    assert.ok(exists('image', image), 'the running container\'s image was removed');
    report.result = 'passed';
  }
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  if (fill) run('rm', ['-f', fill]);
  const left = run('docker', ['ps', '--all', '--quiet', '--filter', `label=${label}`]).stdout?.trim();
  if (left) run('docker', ['rm', '--force', ...left.split(/\s+/)]);
  run('docker', ['image', 'rm', '--force', `stado-test-unused:${process.pid}`]);
  report.finished_at = new Date().toISOString();
  report.scope = 'the janitor Docker cleaner against a real daemon, on a host brought to the disk-full rule';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
