import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the working-directory candidates in `stado service
// list --unowned` and `stado service reap` on this machine. A program started
// inside a managed root with a relative command line names no path under the
// root, so only its working directory places it there. The journey builds a
// dedicated HOME inside this checkout's ignored .build, holding a copy of the
// canonical registry narrowed to this host, so every managed root expands
// under the fixture and no operator process can match. It starts one orphaned
// probe inside `$HOME/.stado/services/<probe>` and an identical one outside
// every root, then checks: the scan reports the inside probe as unowned and
// not the outside one; a reap dry run proposes ending only the inside probe
// and signals nothing; `--apply` ends it and leaves the outside probe alive;
// an empty `--command` is refused. Required: STADO_SERVICE_TEST_HOST, the
// registry name of this machine. STADO_BIN selects the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const host = process.env.STADO_SERVICE_TEST_HOST;
assert.ok(host, 'STADO_SERVICE_TEST_HOST must name this machine in the registry');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'service-cwd-'));
const home = join(output, 'home');
const storage = join(home, '.stado', 'local-storage');
const inside = join(home, '.stado', 'services', 'cwd-probe');
const outside = join(output, 'outside');
const config = join(output, 'config.json');
const stado = process.env.STADO_BIN || 'stado';
const marker = `sleep ${86000 + Math.floor(Math.random() * 399)}`;
const fixture = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: config };
const report = { started_at: new Date().toISOString(), host, marker, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args, env = process.env, cwd = root) {
  const answer = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, home: env.HOME, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.equal(answer.status, 0, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function alive(pid) {
  try { process.kill(Number(pid), 0); return true; } catch { return false; }
}
// The probe's shell exits at once, so the program is reparented to launchd:
// the same shape as a job's leftover child.
function orphan(directory) {
  success(run('/bin/sh', ['-c', `${marker} >/dev/null 2>&1 &`], process.env, directory));
  const found = success(run('/usr/sbin/lsof', ['-nP', '-a', '-d', 'cwd', '-c', 'sleep', '-Fpn']))
    .split('\n').reduce((state, line) => {
      if (line.startsWith('p')) state.pid = line.slice(1);
      if (line.startsWith('n') && line.slice(1) === realpathSync(directory)) state.pids.push(state.pid);
      return state;
    }, { pid: '', pids: [] }).pids;
  assert.equal(found.length, 1, `exactly one probe must run in ${directory}`);
  return found[0];
}
const probes = {};
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--',
    'stado-rs/src/deploy/service', 'stado-rs/src/cli/service', 'tests/service']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  assert.equal(success(run(binary, ['registry', 'self'])).split('\t')[0], host,
    'STADO_SERVICE_TEST_HOST must be this machine: the probes run here');

  const canonical = JSON.parse(success(run(binary, ['registry', 'pull'])));
  canonical.targets = canonical.targets.filter(target => target.name === host);
  assert.equal(canonical.targets.length, 1, `${host} must be in the canonical registry`);
  for (const directory of [storage, inside, outside]) mkdirSync(directory, { recursive: true });
  writeFileSync(join(storage, 'registry.json'), `${JSON.stringify(canonical, null, 2)}\n`, { mode: 0o600 });
  writeFileSync(config, `${JSON.stringify({ providers: ['local'],
    providers_disabled: ['gcp', 'azure', 'aws', 'box'],
    storage: { backend: 'local', local: { path: storage } } }, null, 2)}\n`, { mode: 0o600 });
  probes.inside = orphan(inside);
  probes.outside = orphan(outside);
  report.probes = { ...probes, inside_directory: inside, outside_directory: outside };

  const scan = JSON.parse(success(run(binary, ['service', 'list', '--unowned', '--json'], fixture)));
  report.scan = scan;
  const listed = scan.unowned.filter(process => process.command.includes(marker));
  assert.deepEqual(listed.map(process => process.pid), [probes.inside],
    'Only the probe whose working directory is under a managed root is unowned');
  assert.ok(scan.judged.some(row => row.pid === probes.inside && row.verdict === 'unowned'),
    'The scan must state its verdict on the inside probe');
  assert.ok(scan.searched.some(line => line.includes(join(home, '.stado', 'services'))),
    'The scan must say it searched the fixture services root');

  const reap = ['service', 'reap', '--host', host, '--command', marker, '--json'];
  const dry = JSON.parse(success(run(binary, reap, fixture)));
  report.dry_run = dry;
  assert.deepEqual(dry.reaped.map(row => [row.pid, row.outcome]), [[probes.inside, 'would_end']],
    'A dry run proposes ending only the inside probe');
  assert.ok(alive(probes.inside) && alive(probes.outside), 'A dry run must signal nothing');

  const applied = JSON.parse(success(run(binary, [...reap, '--apply'], fixture)));
  report.applied = applied;
  assert.deepEqual(applied.reaped.map(row => [row.pid, row.outcome]), [[probes.inside, 'ended']],
    '--apply ends only the inside probe');
  assert.equal(alive(probes.inside), false, 'The inside probe is still running');
  assert.equal(alive(probes.outside), true, 'A process outside every managed root was signalled');

  const empty = run(binary, ['service', 'reap', '--host', host, '--command', ' ', '--apply'], fixture);
  assert.notEqual(empty.status, 0, 'An empty --command must be refused');
  assert.match(empty.stderr, /a command substring is required/, 'The refusal must say why');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  for (const pid of Object.values(probes)) if (alive(pid)) process.kill(Number(pid), 'SIGKILL');
  report.finished_at = new Date().toISOString();
  report.scope = 'Working-directory candidates of list --unowned and reap on this macOS host, in a fixture HOME; not Linux /proc, remote hosts or GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
