import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado credentials item show --host <host> <item>`
// reporting an item's context descriptors from the host's own vault: the
// JSON carries a `context` array whose entries are named and, for a scalar,
// valued; the text form prints them as `context: <name>=<value>`; and no
// field value appears in either. Read-only. Required: STADO_ITEM_TEST_HOST,
// a registry host whose installed Stado reports context, and
// STADO_ITEM_TEST_ITEM, an item there that declares at least one scalar
// context entry. STADO_BIN selects the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const host = process.env.STADO_ITEM_TEST_HOST;
const item = process.env.STADO_ITEM_TEST_ITEM;
assert.ok(host && item, 'STADO_ITEM_TEST_HOST and STADO_ITEM_TEST_ITEM are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-item-context-'));
const stado = process.env.STADO_BIN || 'stado';
const report = { started_at: new Date().toISOString(), host, item, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: process.env, encoding: 'utf8' });
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

  const shown = JSON.parse(success(run(binary, ['credentials', 'item', 'show', '--host', host, item, '--json'])));
  assert.ok(Array.isArray(shown.context), `${host} did not report context; its Stado predates context reporting`);
  const scalars = shown.context.filter(entry => entry.value !== null);
  assert.ok(scalars.length > 0, `${item} declares no scalar context entry`);
  for (const entry of shown.context) {
    assert.equal(typeof entry.name, 'string');
    assert.ok(entry.value === null || typeof entry.value !== 'object', `${entry.name} leaked a nested value`);
  }
  report.context_names = shown.context.map(entry => entry.name);

  const text = success(run(binary, ['credentials', 'item', 'show', '--host', host, item]));
  for (const entry of scalars) {
    const value = typeof entry.value === 'string' ? entry.value : JSON.stringify(entry.value);
    assert.ok(text.includes(`context:    ${entry.name}=${value}`), `the text form omits ${entry.name}`);
  }
  for (const field of shown.fields) {
    assert.ok(text.includes(`field:      ${field.name} ${field.length} bytes sha256=${field.sha256}`),
      `the text form omits field ${field.name}`);
  }
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Context descriptors of one real host-vault item in JSON and text, read-only';
  // The report keeps command output, which holds context values (account
  // addresses), so it is owner-only like every other report here.
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
