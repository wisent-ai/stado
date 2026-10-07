import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the workspace a product sync reads: `stado product
// --workspace DIR sync` works in DIR, not in WISENT_WORKSPACE or
// ~/Documents/CodingProjects/Wisent, which is what lets the host process's
// product sync run in ~/.stado/products/workspace, a directory macOS does not
// withhold from background programs.
//
// The workspace is a dedicated empty directory below this checkout's ignored
// .build, and WISENT_WORKSPACE names a second one, so a sync that ignored
// --workspace would name the wrong directory. `--dry-run` decides without
// fetching, building or installing, so the machine's installations are not
// touched; with `--clone-missing` it still clones nothing. STADO_BIN selects
// the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-sync-workspace-'));
const stado = process.env.STADO_BIN || 'stado';
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args, env = process.env, cwd = root) {
  const answer = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.equal(answer.status, 0, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  const workspace = join(output, 'named');
  const decoy = join(output, 'environment');
  mkdirSync(workspace);
  mkdirSync(decoy);
  const env = { ...process.env, WISENT_WORKSPACE: decoy };

  const planned = run(binary, ['product', '--workspace', workspace, 'sync', '--surface', 'cli',
    '--clone-missing', '--dry-run', '--json'], env);
  const rows = JSON.parse(success(planned));
  report.rows = rows;
  assert.ok(Array.isArray(rows) && rows.length > 0, 'The sync must decide every catalogued CLI surface');
  const missing = rows.filter(row => row.status === 'no-checkout');
  assert.ok(missing.length > 0, 'An empty workspace holds no checkout of any product');
  for (const row of missing) {
    assert.equal(row.decision, 'held', `${row.product}: a product without a checkout is held on a dry run`);
    assert.ok(String(row.detail).includes(workspace), `${row.product}: the refusal must name the --workspace directory: ${row.detail}`);
    assert.ok(!String(row.detail).includes(decoy), `${row.product}: WISENT_WORKSPACE must not be read when --workspace is named`);
  }
  assert.deepEqual(readdirSync(workspace), [], 'A dry run must clone nothing, --clone-missing included');
  assert.deepEqual(readdirSync(decoy), [], 'Nothing may be written to the workspace --workspace replaced');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'stado product --workspace selection for sync and the dry run of --clone-missing; no fetch, build or installation';
  for (const name of ['named', 'environment']) {
    rmSync(join(output, name), { recursive: true, force: true });
  }
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
