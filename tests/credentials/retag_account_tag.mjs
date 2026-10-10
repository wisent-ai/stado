import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado credentials item retag --host <host> <item>
// --tags <tags>` accepting the tags an item already carries when one of them
// is an account address (`brama:account:<address>`, written by Brama through
// Skarbiec): the command reads the item's tags, writes exactly the same set
// back and reports the same set after. Before the alphabet admitted `@`, this
// round trip was refused with "tag must contain only letters, digits, '.',
// '_', '-' or ':'", so a `brama:login:` correction had to drop the account
// first. The item ends with the tags it started with. Required:
// STADO_RETAG_TEST_HOST, a registry host owning a vault,
// STADO_RETAG_TEST_ITEM, an item there carrying a `brama:account:` tag, and
// STADO_BIN, the Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const host = process.env.STADO_RETAG_TEST_HOST;
const item = process.env.STADO_RETAG_TEST_ITEM;
const stado = process.env.STADO_BIN;
assert.ok(host && item && stado,
  'STADO_RETAG_TEST_HOST, STADO_RETAG_TEST_ITEM and STADO_BIN are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'credentials-retag-account-'));
const report = { started_at: new Date().toISOString(), host, item, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: process.env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function tagsOf(binary) {
  const shown = JSON.parse(success(run(binary, ['credentials', 'item', 'show', '--host', host, item, '--json'])));
  assert.ok(Array.isArray(shown.tags), `${host} did not report tags for ${item}`);
  return shown.tags;
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  const before = tagsOf(binary);
  const account = before.find(tag => tag.startsWith('brama:account:'));
  assert.ok(account, `${item} carries no brama:account: tag; name an item Brama attributed`);
  assert.ok(account.includes('@'), `${account} holds no address, so the alphabet is not exercised`);
  report.tags_before = before;

  const written = success(run(binary, ['credentials', 'item', 'retag', '--host', host, item, '--tags', before.join(',')]));
  assert.ok(written.includes('now rev='), `retag reported no "now" line: ${written}`);
  const after = tagsOf(binary);
  assert.deepEqual(after, before, 'the round trip changed the tag set');
  report.tags_after = after;
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Round-trip retag of one real host-vault item whose tags carry an account address';
  // Command output names account addresses, so the report is owner-only.
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`,
    { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
