import { createHash, randomUUID } from 'node:crypto';
import { closeSync, mkdirSync, openSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const directory = join(root, 'build/database-notifications', `build-${randomUUID()}`);
mkdirSync(directory, { recursive: true });
const report = { outcome: 'failed', commands: [], runnerSha256: digest(readFileSync(fileURLToPath(import.meta.url))) };
function digest(bytes) { return createHash('sha256').update(bytes).digest('hex'); }
function save() { writeFileSync(join(directory, 'build-report.json'), JSON.stringify(report)); }
function required(name) {
  const value = process.env[name];
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${name} must name a qualification input`);
  return value;
}
function command(program, args, environment = process.env) {
  const identity = randomUUID();
  const stdoutPath = join(directory, `${identity}.stdout`);
  const stderrPath = join(directory, `${identity}.stderr`);
  const stdout = openSync(stdoutPath, 'w');
  let result;
  try {
    const stderr = openSync(stderrPath, 'w');
    try {
      result = spawnSync(program, args, { cwd: root, env: environment, stdio: ['ignore', stdout, stderr] });
    } finally { closeSync(stderr); }
  } finally { closeSync(stdout); }
  report.commands.push({ program, args, stdoutPath, stderrPath, status: result.status, signal: result.signal,
    startError: result.error?.message });
  save();
  if (result.error) throw new Error(`${program} could not start: ${result.error.message}`);
  if (result.signal || result.status !== successExit) {
    throw new Error(`${program} ${JSON.stringify(args)} ended with status ${result.status}, signal ${result.signal}; see ${stderrPath}`);
  }
  return readFileSync(stdoutPath, 'utf8');
}
function cleanSource() {
  const revision = command('git', ['rev-parse', 'HEAD']).trim();
  if (revision !== sourceRevision) throw new Error(`HEAD ${revision} differs from STADO_TEST_SOURCE_REVISION ${sourceRevision}`);
  command('git', ['diff', '--exit-code', 'HEAD', '--', 'stado-rs', 'tests/database/notifications']);
  const untracked = command('git', ['ls-files', '--others', '--exclude-standard', '--', 'stado-rs', 'tests/database/notifications']);
  if (untracked.trim()) throw new Error(`qualification source contains untracked files: ${untracked}`);
}
let successExit;
let sourceRevision;
save();
try {
  sourceRevision = required('STADO_TEST_SOURCE_REVISION');
  successExit = Number(required('STADO_TEST_SUCCESS_EXIT'));
  if (!Number.isInteger(successExit)) throw new Error('STADO_TEST_SUCCESS_EXIT must be an integer process success status');
  const cargo = required('STADO_TEST_CARGO');
  report.sourceRevision = sourceRevision;
  report.database = required('STADO_TEST_NOTIFICATION_DATABASE');
  report.fleetHome = required('STADO_TEST_NOTIFICATION_FLEET_HOME');
  save();
  cleanSource();
  const compiled = command(cargo, ['test', '--manifest-path', 'stado-rs/database/Cargo.toml',
    '--locked', '--test', 'notifications', '--no-run', '--message-format=json']);
  let executable;
  for (const line of compiled.split('\n').filter(line => line.trim())) {
    const message = JSON.parse(line);
    if (message.reason === 'compiler-artifact' && message.target?.name === 'notifications' && message.profile?.test && message.executable) {
      if (executable && executable !== message.executable) throw new Error('Cargo reported different notification test executables');
      executable = message.executable;
    }
  }
  if (!executable) throw new Error('Cargo did not report the compiled notifications test executable');
  cleanSource();
  const manifest = { source_revision: sourceRevision, binary_sha256: digest(readFileSync(executable)) };
  const manifestPath = join(directory, 'build-manifest.json');
  writeFileSync(manifestPath, JSON.stringify(manifest));
  report.candidate = { executable, manifestPath, ...manifest };
  save();
  command(executable, ['--nocapture'], { ...process.env, STADO_TEST_BUILD_MANIFEST: manifestPath });
  report.outcome = 'passed';
} catch (cause) {
  report.error = String(cause);
  throw cause;
} finally {
  save();
  console.error(`notification qualification evidence: ${directory}`);
}
