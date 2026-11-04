import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// The network node launchd starts takes its port back from a copy an
// `@reboot` cron line started from the unit's old fallback wrapper, and
// removes that line, so the next boot starts one node. Run only on a
// dedicated macOS account whose node was brought up by `stado network up`
// (its unit com.wisent.stado-network loaded) and whose crontab belongs to
// this journey: it adds the fallback line and starts the cron copy itself,
// the way the old install did, then restarts the unit. Never on an
// operator's account.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = process.env.STADO_BIN;
assert.ok(binary, 'STADO_BIN names the stado under test');
assert.equal(process.env.STADO_TEST_DEDICATED_ACCOUNT, '1',
  'STADO_TEST_DEDICATED_ACCOUNT=1 says this account and its crontab belong to the journey');
assert.equal(process.platform, 'darwin', 'This journey drives launchd');
const program = realpathSync(binary);
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'network-cron-copy-'));
const report = { commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const home = process.env.HOME;
const label = 'com.wisent.stado-network';
const wrapper = join(home, '.stado', 'bin', `run-${label}.sh`);
const log = join(home, '.stado', 'logs', `${label}.log`);

function run(executable, args, options = {}) {
  const answer = spawnSync(executable, args, { cwd: root, encoding: 'utf8', ...options });
  report.commands.push({ executable, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function succeeded(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function listener(port) {
  const listed = run('/usr/sbin/lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-Fp']).stdout;
  return listed.split('\n').filter(line => line.startsWith('p')).map(line => Number(line.slice(1)));
}
function unitPid() {
  const shown = run('launchctl', ['print', `gui/${process.getuid()}/${label}`]).stdout;
  const match = shown.match(/^\s*pid = (\d+)/m);
  return match ? Number(match[1]) : undefined;
}

let failure;
try {
  report.source_revision = succeeded(run('git', ['-C', root, 'rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: program, sha256: digest(readFileSync(program)) };
  succeeded(run(program, ['network', 'up']));
  const node = JSON.parse(readFileSync(join(home, '.stado', 'network', 'node.json'), 'utf8'));
  const plist = join(home, 'Library', 'LaunchAgents', `${label}.plist`);
  assert.ok(existsSync(plist), 'network up loaded the node unit');

  // The old install's fallback: a wrapper running the node, a cron line
  // starting it at boot, and the copy it started, which holds the port.
  succeeded(run('launchctl', ['bootout', `gui/${process.getuid()}/${label}`]));
  writeFileSync(wrapper, `#!/bin/sh\nexport HOME=${home}\nexec ${join(home, '.stado', 'libexec', 'stado-network')} network serve\n`);
  const before = run('crontab', ['-l']).stdout || '';
  const line = `@reboot /bin/sh ${wrapper} >> ${log} 2>&1`;
  succeeded(run('crontab', ['-'], { input: `${before.trimEnd()}\n${line}\n`.trimStart() }));
  run('/bin/sh', ['-c', `nohup /bin/sh ${wrapper} >> ${log} 2>&1 </dev/null &`]);
  let copy;
  for (let tries = 0; tries < 50 && !copy; tries += 1) {
    [copy] = listener(node.port);
    if (!copy) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);
  }
  assert.ok(copy, `the cron copy did not listen on ${node.port}`);
  report.cron_copy = copy;

  // The unit starts its node; that node must take the port back.
  succeeded(run('launchctl', ['bootstrap', `gui/${process.getuid()}`, plist]));
  const status = JSON.parse(succeeded(run(program, ['network', 'status', '--json'])));
  report.status = status;
  const pid = unitPid();
  assert.ok(pid, 'launchd runs no node after the bootstrap');
  assert.deepEqual(listener(node.port), [pid], 'the unit\'s node does not hold the port alone');
  assert.equal(run('kill', ['-0', String(copy)]).status !== 0, true, 'the cron copy still runs');
  assert.ok(!(run('crontab', ['-l']).stdout || '').includes(wrapper), 'the cron line was not removed');
  assert.ok(!existsSync(wrapper), 'the cron wrapper was not removed');
  assert.match(readFileSync(log, 'utf8'), new RegExp(`port ${node.port} was held by pid ${copy}, a copy of this node cron started`));
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Dedicated macOS account: real launchd node unit, a real cron line and the copy it started; no simulated process.';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
