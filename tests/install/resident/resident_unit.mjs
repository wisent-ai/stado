import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Run only on a dedicated release host/account with its native resident Stado
// unit still carrying an argv the incoming parser rejects. This uses the real
// CLI, canonical source checkout, launchd/systemd unit and registry, and leaves
// the upgraded service running. Never point it at an operator's live account.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const binary = process.env.STADO_BIN;
const source = process.env.STADO_TEST_SOURCE;
const commit = process.env.STADO_TEST_SOURCE_COMMIT;
const host = process.env.STADO_TEST_DEDICATED_HOST;
assert.ok(binary && source && commit && host,
  'STADO_BIN, STADO_TEST_SOURCE, STADO_TEST_SOURCE_COMMIT and STADO_TEST_DEDICATED_HOST name the dedicated journey');
const program = realpathSync(binary);
const checkout = realpathSync(source);
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-resident-unit-'));
const report = { source: checkout, commit, host, commands: [], cases: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function run(executable, args) {
  const answer = spawnSync(executable, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ executable, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function succeeded(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}

let failure;
try {
  report.source_revision = succeeded(run('git', ['-C', checkout, 'rev-parse', 'HEAD']));
  assert.equal(commit, report.source_revision, 'the test binds installation to the committed source');
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.binary = { path: program, sha256: digest(readFileSync(program)),
    version: succeeded(run(program, ['--version'])) };
  const self = succeeded(run(program, ['registry', 'self', '--local', '--name-only']));
  assert.equal(self, host, 'the dedicated host must be this machine, not a remote production unit');
  const installed = join(process.env.HOME, '.stado', 'bin', 'stado');
  assert.ok(existsSync(installed), 'the dedicated account has a predecessor installed');
  report.before_sha256 = digest(readFileSync(installed));

  const rejected = run(program, ['quality', 'parse', '--', 'serve', '--disk-cleanup']);
  assert.ok(rejected.status && !rejected.error && !rejected.signal,
    'the incoming real parser must reject the retired unit role');
  assert.match(rejected.stderr, /disk-cleanup/);
  const unit = ['quality', 'units', '--candidate', program, '--program', 'stado', '--host', host];
  const before = run(program, unit);
  assert.ok(before.status && !before.error && !before.signal,
    'the installed native unit must reproduce the candidate argv refusal');
  assert.match(before.stderr, /disk-cleanup/);
  assert.match(before.stderr, /com\.wisent\.stado/);
  report.cases.push({ case: 'native-unit-refused-before-migration', refusal: before.stderr });

  const workspace = dirname(checkout);
  const install = run(program, ['product', '--workspace', workspace, 'install', 'stado',
    '--surface', 'cli', '--source-commit', commit]);
  succeeded(install);
  assert.match(install.stderr + install.stdout, /migrated com\.wisent\.stado/);
  const after = run(program, unit);
  succeeded(after);
  const status = JSON.parse(succeeded(run(program, ['product', '--workspace', workspace,
    'status', 'stado', '--surface', 'cli', '--json'])));
  assert.equal(status.source_revision, commit, 'the installed receipt names the source actually built');
  const serving = JSON.parse(succeeded(run(program, ['service', 'serving', 'stado', '--host', host, '--wait', '--json'])));
  assert.ok(Array.isArray(serving) && serving.length && serving.every(row => row.serving === 'serving'),
    'the real resident process owns every declared port after the install');
  const service = JSON.parse(succeeded(run(program, ['service', 'status', 'stado', '--json'])));
  assert.ok(service.some(row => row.host === host && row.name === 'stado'),
    'the product retains the real resident unit row, even before its next beacon');
  report.after_sha256 = digest(readFileSync(installed));
  // The resident unit runs the binary this install placed, not the image it
  // was started on before: the inode its process executes is the installed
  // file's.
  const unitShown = JSON.parse(succeeded(run(program, ['service', 'unit', 'show',
    'com.wisent.stado', '--host', host, '--json'])));
  assert.ok(unitShown.pid, `the resident unit runs no process after the install: ${JSON.stringify(unitShown)}`);
  const listed = succeeded(run('lsof', ['-a', '-p', String(unitShown.pid), '-d', 'txt', '-Fi']));
  const [runningInode] = listed.split('\n').filter(line => line.startsWith('i')).map(line => line.slice('i'.length));
  const installedInode = String(statSync(realpathSync(installed)).ino);
  assert.equal(runningInode, installedInode,
    'the resident unit still executes the image the install replaced');
  report.cases.push({ case: 'migrated-and-serving', status, serving, service, unit: unitShown, image_inode: runningInode });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Dedicated real resident unit and canonical source install; the pre-existing unit must carry the retired argv. No simulated provider or dry run.';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
