import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// A network node declares what its network requires of the host's
// connection: the real `stado market node add` refuses a figure without its
// source, a source without a figure and a figure at or below zero; a node
// declared with requirements keeps them in the fleet's store, `show --json`
// reads them back, `edit` replaces the set, and the node is removed again.
// Then `follow --dry-run --once` on a node whose upload floor no host
// measures says it is not started for the bandwidth. STADO_BIN selects the
// Stado under test; STADO_TEST_TARGET a registry host and STADO_TEST_SERVICE
// a service declared on it (the operator's, nothing assumed). Without them
// the live half is reported blocked.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'market-node-requirements-'));
const stado = process.env.STADO_BIN;
const target = process.env.STADO_TEST_TARGET;
const service = process.env.STADO_TEST_SERVICE;
const name = `req-test-${process.pid}`;
const page = 'https://example.org/requirements-under-test';
const report = { started_at: new Date().toISOString(), node: name, target, service, commands: [], result: 'failed' };
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
function refused(answer, word) {
  assert.ok(!exitedCleanly(answer), `accepted: ${answer.stdout}`);
  assert.ok(`${answer.stderr}${answer.stdout}`.includes(word), `the refusal does not name ${word}: ${answer.stderr}`);
}
let added = false;
let binary;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado) {
    report.result = 'blocked';
    throw new Error('requires STADO_BIN, the Stado under test');
  }
  report.stado = snapshotSubject(stado, output);
  binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);
  if (!target || !service) {
    report.result = 'blocked';
    throw new Error('requires STADO_TEST_TARGET and STADO_TEST_SERVICE, a registry host and a service declared on it');
  }
  const add = extra => run(binary, ['market', 'node', 'add', name, '--source', 'mysterium', '--target', target,
    '--service', service, '--cpu-cores', '1', '--ram-gb', '1', '--vram-gb', '0', ...extra, '--json']);

  refused(add(['--min-upload-mbps', '20']), '--requirements-source');
  refused(add(['--requirements-source', page]), 'none was given');
  refused(add(['--min-upload-mbps', '0', '--requirements-source', page]), 'above zero');

  // No host measures a connection this fast, so the dry run must not start it.
  success(add(['--min-upload-mbps', '1000000', '--requirements-source', page]));
  added = true;
  let shown = JSON.parse(success(run(binary, ['market', 'node', 'show', name, '--json'])));
  assert.deepEqual(shown.node.requirements, { min_upload_mbps: 1000000, source: page });

  success(run(binary, ['market', 'node', 'edit', name, '--min-download-mbps', '1000000',
    '--requirements-source', page, '--listed', 'true', '--json']));
  shown = JSON.parse(success(run(binary, ['market', 'node', 'show', name, '--json'])));
  assert.deepEqual(shown.node.requirements, { min_download_mbps: 1000000, source: page },
    'edit did not replace the whole set');

  const preview = success(run(binary, ['market', 'node', 'follow', name, '--idle-window-s', '0',
    '--dry-run', '--once']));
  report.preview = preview;
  assert.match(preview, /Mbit\/s|bandwidth/, `the preview does not name the bandwidth: ${preview}`);
  assert.doesNotMatch(preview, /would start/, `the node would start below its network's floor: ${preview}`);

  success(run(binary, ['market', 'node', 'remove', name, '--json']));
  added = false;
  refused(run(binary, ['market', 'node', 'show', name, '--json']), name);
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  if (added) run(binary, ['market', 'node', 'remove', name, '--json']);
  report.finished_at = new Date().toISOString();
  report.scope = "a network node's connection requirements: refusals, stored set, replacement, the follow loop withholding the node, removal";
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
