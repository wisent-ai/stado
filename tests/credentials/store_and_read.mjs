import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of the daily secrets loop the docs' example
// (examples/secrets-store-and-read.sh) shows: one value stored in Skarbiec
// through `stado credentials put`, read back with one command, listed, then
// removed, and a read after removal refused. The item is a fresh random name
// no other item can carry, so no operator item is touched, and the value
// travels on standard input only.
// Required: STADO_BIN, the Stado under test (a path or a program on PATH),
// and the machine's own configured vault (the one `stado credentials vault show`
// names).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN must name the Stado under test, a path or a program on PATH');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-store-and-read-'));
const item = `qualification-secret-${randomUUID()}`;
const value = randomUUID();
const report = { started_at: new Date().toISOString(), item, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const succeeded = answer => answer.status !== null && !answer.status;
// The value never reaches the report: only its digest does.
function run(program, args, input) {
  const answer = spawnSync(program, args, { cwd: root, env: process.env, encoding: 'utf8', input });
  report.commands.push({ program, arguments: args, stdin_sha256: input && digest(input), exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout_sha256: digest(answer.stdout), stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
let stored = false;
let binary;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  report.vault = success(run(binary, [...'credentials vault'.split(' '), 'show']));

  success(run(binary, ['credentials', 'put', item], value));
  stored = true;
  assert.equal(success(run(binary, ['credentials', 'get', item])), value, 'the value read back is the value stored');
  assert.ok(success(run(binary, ['credentials', 'ls'])).includes(item), 'the stored item is listed');

  success(run(binary, ['credentials', 'rm', item]));
  stored = false;
  const gone = run(binary, ['credentials', 'get', item]);
  assert.ok(!succeeded(gone), 'a removed item is not read back');
  assert.ok(gone.stderr.includes(item), `the refusal names the item: ${gone.stderr}`);
  assert.equal(gone.stdout.trim(), '', 'a refused read prints no value');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  throw error;
} finally {
  if (stored && binary) report.cleanup = run(binary, ['credentials', 'rm', item]).status;
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(join(output, 'report.json'));
}
