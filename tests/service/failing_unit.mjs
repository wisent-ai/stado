import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Qualification of `stado service unit show` naming a unit its supervisor
// holds loaded with no process after a failed run: a real launchd job in this
// account's GUI domain whose program exits with the code the caller states,
// under a label unique to this run, its plist inside this checkout's ignored
// .build. Case one: once launchd reports the run ended, `unit show --json`
// carries `failing` with that exit code and the run count, and the text row
// `failing` says the same. Case two: a job whose program is still running
// carries no `failing`. Both jobs are booted out at the end, and `unit show`
// then refuses the label as not loaded. Required: STADO_BIN, the Stado under
// test; STADO_SERVICE_TEST_HOST, this machine's registry name; and
// STADO_SERVICE_TEST_EXIT_CODE, the nonzero code the failing job exits with.
// macOS only.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: it names the Stado whose service unit show this journey qualifies');
const host = process.env.STADO_SERVICE_TEST_HOST;
assert.ok(host, 'STADO_SERVICE_TEST_HOST must name this machine as the registry does');
const exitCode = process.env.STADO_SERVICE_TEST_EXIT_CODE;
assert.ok(Number.isInteger(Number(exitCode)) && Number(exitCode),
  'STADO_SERVICE_TEST_EXIT_CODE must be the nonzero code the failing job exits with');
assert.equal(process.platform, 'darwin', 'This journey drives launchd');
const output = mkdtempSync(join(build, 'service-failing-unit-'));
const domain = `gui/${process.getuid()}`;
const report = { started_at: new Date().toISOString(), exit_code: exitCode, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const succeeded = answer => answer.status !== null && !answer.status;
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function plist(label, script) {
  const path = join(output, `${label}.plist`);
  writeFileSync(path, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>${label}</string>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>${script}</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><false/>
</dict></plist>
`);
  return path;
}
const runId = output.split('-').pop();
const failingLabel = `com.wisent.stado-test.failing-${runId}`;
const runningLabel = `com.wisent.stado-test.running-${runId}`;
const fifo = join(output, 'release');
const ending = join(output, 'ending');
const exited = new RegExp(`runs no process: its last run exited ${exitCode}\\b`);
let failure;
let binary;
const loaded = [];
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const show = (label, json) => run(binary, ['service', 'unit', 'show', label, '--host', host, ...(json ? ['--json'] : [])]);

  // The running job blocks on a FIFO nobody writes until the journey ends.
  success(run('mkfifo', [fifo]));
  success(run('mkfifo', [ending]));
  success(run('launchctl', ['bootstrap', domain, plist(runningLabel, `read line < ${fifo}`)]));
  loaded.push(runningLabel);
  // The failing job says it is about to exit on its own FIFO; reading that
  // FIFO blocks until it does, so the journey waits on the job's event.
  success(run('launchctl', ['bootstrap', domain, plist(failingLabel, `echo ending > ${ending}; exit ${exitCode}`)]));
  loaded.push(failingLabel);
  readFileSync(ending);

  // Between the job's last word and launchd reaping it, the supervisor may
  // still hold its pid; every read is the product's own and lands in the
  // report.
  let failed;
  for (;;) {
    failed = JSON.parse(success(show(failingLabel, true)));
    if (!failed.pid && failed.last_exit_code) break;
  }
  report.failing = failed;
  assert.ok(failed.failing, `${failingLabel} ended with ${failed.last_exit_code} and runs no process, yet no failing verdict`);
  assert.match(failed.failing, exited, 'The verdict must carry the exit code');
  assert.match(failed.failing, new RegExp(`started it ${failed.runs} time`), 'The verdict must carry the supervisor run count');
  const text = success(show(failingLabel, false));
  assert.ok(text.split('\n').some(line => line.trimStart().startsWith('failing') && exited.test(line)),
    'The text answer must print the failing row');

  const alive = JSON.parse(success(show(runningLabel, true)));
  report.running = alive;
  assert.ok(alive.pid, `${runningLabel} must be running while its FIFO is unwritten`);
  assert.equal(alive.failing, null, 'A running unit is never reported failing');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  for (const label of loaded) {
    run('launchctl', ['bootout', `${domain}/${label}`]);
  }
  if (binary) {
    const gone = run(binary, ['service', 'unit', 'show', failingLabel, '--host', host]);
    report.after_bootout = { exit_status: gone.status, stderr: gone.stderr };
    if (succeeded(gone) && !failure) {
      failure = new Error(`${failingLabel} still answers after its bootout`);
      report.result = 'failed';
    }
  }
  report.finished_at = new Date().toISOString();
  report.scope = 'Real launchd jobs in this account\'s GUI domain read through stado service unit show; not systemd, not the beacon or the delivery restart';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) {
  throw failure;
}
