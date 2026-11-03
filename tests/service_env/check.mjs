import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado service env check` against a managed service's
// env file on STADO_ENV_TEST_HOST. The check must read the file on that host
// (the remote script runs under the host's own /bin/bash, 3.2 on macOS) and
// answer with every loopback endpoint the file declares and who listens there;
// before the repair every Mac refused the script with "unexpected EOF while
// looking for matching". The refusal half: a path outside the account's home
// is answered as `refused_outside_home`, not read. Read only; nothing on the
// host is changed. Required: STADO_BIN, STADO_ENV_TEST_HOST,
// STADO_ENV_TEST_SERVICE, STADO_ENV_TEST_FILE ($HOME/... of an env file that
// service reads).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const host = required('STADO_ENV_TEST_HOST');
const service = required('STADO_ENV_TEST_SERVICE');
const file = required('STADO_ENV_TEST_FILE');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'service-env-check-'));
const report = { started_at: new Date().toISOString(), host, service, file, commands: [], result: 'failed' };
report.stado_sha256 = createHash('sha256').update(readFileSync(stado)).digest('hex');

function check(envFile) {
  const answer = spawnSync(stado, ['service', 'env', 'check', service, '--host', host, '--env-file', envFile, '--json'],
    { encoding: 'utf8', timeout: 300_000 });
  const record = { env_file: envFile, status: answer.status, stdout: answer.stdout,
    stderr_tail: (answer.stderr || '').slice(-3000) };
  report.commands.push(record);
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  return record;
}

try {
  const read = check(file);
  assert.doesNotMatch(read.stderr_tail, /unexpected EOF|syntax error/, 'the host shell refused the check script');
  const [answer] = JSON.parse(read.stdout);
  assert.ok(answer, `the check printed no answer for ${host}: ${read.stderr_tail}`);
  assert.equal(answer.file_state ?? answer.env_file?.file_state, 'read', 'the env file was read on the host');
  assert.equal(answer.listeners_state ?? answer.env_file?.listeners_state, 'read', 'the socket table was read with owners');

  const outside = check('/etc/hosts');
  const [refused] = JSON.parse(outside.stdout || '[]');
  assert.equal(refused?.file_state ?? refused?.env_file?.file_state, 'refused_outside_home',
    `a path outside the home must be refused, not read: ${outside.stderr_tail}`);
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`report: ${join(output, 'report.json')} (${report.result})`);
}
