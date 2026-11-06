import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly } from '../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// What each source earned a host per board-hour: the real `stado market
// sources --target HOST --json` reads the fleet's kept payouts, node runs and
// Vast.ai earnings days for a registry host, and every source it values
// either carries a value with the hours it is read over or says why it has
// none; Vast.ai is always one of them. Nothing is written to the fleet's
// store. The refusal needs no host: a --target the registry does not declare
// is refused with class not_found. STADO_BIN selects the Stado under test;
// STADO_TEST_TARGET a registry host to read (the operator's, nothing
// assumed). Without it the live half is reported blocked.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'market-source-value-'));
const stado = process.env.STADO_BIN;
const target = process.env.STADO_TEST_TARGET;
const report = { started_at: new Date().toISOString(), target, commands: [], result: 'failed' };
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
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado) {
    report.result = 'blocked';
    throw new Error('requires STADO_BIN, the Stado under test');
  }
  report.stado = snapshotSubject(stado, output);
  const binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);

  // A host the registry does not declare is refused, not valued at nothing.
  const unknown = run(binary, ['market', 'sources', '--target', 'no-such-host-for-source-value', '--json']);
  assert.ok(!exitedCleanly(unknown), `accepted: ${unknown.stdout}`);
  assert.ok(`${unknown.stderr}${unknown.stdout}`.includes('not_found'),
    `the refusal does not state not_found: ${unknown.stderr}`);

  if (!target) {
    report.result = 'blocked';
    throw new Error('the live reading requires STADO_TEST_TARGET, a registry host');
  }
  const answer = JSON.parse(success(run(binary, ['market', 'sources', '--target', target, '--json'])));
  report.answer = answer;
  assert.equal(answer.target, target);
  if (answer.per_board_hour_unread) {
    report.result = 'blocked';
    throw new Error(`no pricing window to read the values over: ${answer.per_board_hour_unread}`);
  }
  const { window_seconds: windowSeconds, values } = answer.per_board_hour;
  assert.ok(Number.isInteger(windowSeconds) && windowSeconds > 0, `window ${windowSeconds}`);
  assert.ok(values.some(value => value.source === 'vast'), 'Vast.ai is not among the values');
  for (const value of values) {
    if (value.usd_per_hour === null) {
      assert.ok(typeof value.unread === 'string' && value.unread.length > 0,
        `${value.source} has neither a value nor a reason: ${JSON.stringify(value)}`);
    } else {
      assert.ok(Number.isFinite(value.usd_per_hour) && value.usd_per_hour >= 0, JSON.stringify(value));
      assert.ok(Number.isFinite(value.hours) && value.hours > 0,
        `${value.source} is valued over no hours: ${JSON.stringify(value)}`);
    }
  }
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = "each source's value per board-hour on a registry host, read from what the fleet keeps, and the refusal of an undeclared host";
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
