import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado host run attach` against a program that leaves
// a detached child holding its output, as `release local install` does when it
// restarts a service. The Stado under test delivers a two-line program into a
// managed run tree on STADO_ATTACH_TEST_HOST and attaches to it. The program
// prints a line, starts a child that keeps stdout and stderr open for longer
// than the bound below, and exits 3. The attach must return within
// STADO_ATTACH_TEST_BOUND_SECONDS with exit status 3, print the program's line,
// and name the child's pid as still holding the output; before the repair it
// waited for the child and ended with "attached process signal marker is
// unavailable". The run tree is removed afterwards. Required: STADO_BIN,
// STADO_ATTACH_TEST_HOST, STADO_ATTACH_TEST_HOME (the home of the account
// Stado reaches on that host, where the delivery's $HOME resolves) and
// STADO_ATTACH_TEST_BOUND_SECONDS.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const host = required('STADO_ATTACH_TEST_HOST');
const remoteHome = required('STADO_ATTACH_TEST_HOME');
const bound = Number(required('STADO_ATTACH_TEST_BOUND_SECONDS'));
assert.ok(bound > 0, 'STADO_ATTACH_TEST_BOUND_SECONDS must be a positive number');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'host-run-attach-'));
const report = { started_at: new Date().toISOString(), host, bound, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
report.stado_sha256 = digest(readFileSync(stado));

function run(args, timeoutSeconds) {
  const started = Date.now();
  const answer = spawnSync(stado, args, { encoding: 'utf8', timeout: timeoutSeconds * 1000 });
  const record = { args, status: answer.status, signal: answer.signal, seconds: (Date.now() - started) / 1000,
    stdout: answer.stdout, stderr_tail: (answer.stderr || '').slice(-3000) };
  report.commands.push(record);
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  return record;
}

const runId = crypto.randomUUID();
const program = join(output, 'leaves-a-child.sh');
const childSeconds = Math.ceil(bound * 3);
writeFileSync(program, `#!/bin/bash\nprintf 'program line\\n'\n( /bin/sleep ${childSeconds} ) &\nprintf '%s\\n' "child $!" >&2\nexit 3\n`);
chmodSync(program, 0o755);
const remote = `.stado/work/runs/${runId}/leaves-a-child.sh`;
let delivered = false;
try {
  const deliver = run(['host', 'run', 'deliver', host, program, remote, '--json'], 300);
  assert.equal(deliver.status, 0, deliver.stderr_tail);
  delivered = true;
  const destination = JSON.parse(deliver.stdout).destination;
  assert.equal(destination, `$HOME/${remote}`, `the delivery landed elsewhere: ${destination}`);
  const absolute = `${remoteHome}/${remote}`;
  report.absolute = absolute;

  const attached = run(['host', 'run', 'attach', host, '--program', absolute], bound * 4);
  assert.notEqual(attached.signal, 'SIGTERM', `the attach was still waiting after ${bound * 4}s`);
  assert.ok(attached.seconds <= bound, `the attach returned after ${attached.seconds}s, past the ${bound}s bound`);
  assert.equal(attached.status, 3, `the attach must return the program's own status: ${attached.stderr_tail}`);
  assert.match(attached.stdout, /program line/, 'the program\'s own output is relayed');
  const child = /child (\d+)/.exec(attached.stderr_tail)?.[1];
  assert.ok(child, `the program's child pid is not in its output: ${attached.stderr_tail}`);
  assert.match(attached.stderr_tail, new RegExp(`exited with status 3; .*${child}`),
    'the attach names the child still holding the output');
  report.result = 'passed';
} finally {
  if (delivered) run(['host', 'run', 'remove', host, `.stado/work/runs/${runId}`], 300);
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`report: ${join(output, 'report.json')} (${report.result})`);
}
