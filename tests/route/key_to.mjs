import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado route key TARGET --to HOST` against the live
// registry. TARGET's resolver key is established on TARGET and authorized on
// HOST through the host channel; a second run must find both in place and
// write nothing. A HOST that is TARGET itself is refused, and so is the
// directory authority as TARGET without --to, each naming the cause. The
// authorization appends one public key line to HOST's authorized_keys and is
// what a vault move off TARGET onto HOST needs anyway. Required: STADO_BIN
// (the Stado under test), STADO_ROUTE_KEY_TEST_TARGET (a registry host) and
// STADO_ROUTE_KEY_TEST_TO (another registry host TARGET's resolver tunnels
// to).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
const target = process.env.STADO_ROUTE_KEY_TEST_TARGET;
const to = process.env.STADO_ROUTE_KEY_TEST_TO;
assert.ok(stado && target && to,
  'STADO_BIN, STADO_ROUTE_KEY_TEST_TARGET and STADO_ROUTE_KEY_TEST_TO are required');
assert.notEqual(target, to, 'STADO_ROUTE_KEY_TEST_TO must name another host than the target');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'route-key-to-'));
const report = { started_at: new Date().toISOString(), target, to, cases: [], commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(!answer.status && !answer.signal && !answer.error,
    answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function refused(answer, pattern, what) {
  assert.ok(answer.status && !answer.error && !answer.signal, `${what} must be refused`);
  assert.match(answer.stderr, pattern);
  return answer.stderr;
}

let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  const self = run(binary, ['route', 'key', target, '--to', target]);
  report.cases.push({ case: 'to-itself', refusal: refused(self,
    new RegExp(`--to "${target}" is "${target}" itself, and a resolver opens no session to its own host`),
    'a key authorized on its own host') });

  const first = JSON.parse(success(run(binary, ['route', 'key', target, '--to', to, '--json'])));
  const key = first.resolver_key;
  assert.equal(key.target, target);
  assert.equal(key.authorized_on, to, 'the key is authorized on the host --to names');
  assert.ok(['minted', 'present'].includes(key.key_state), `key_state ${key.key_state}`);
  assert.ok(['authorized', 'already_present'].includes(key.authorized_keys), `authorized_keys ${key.authorized_keys}`);
  report.cases.push({ case: 'authorize', receipt: first });

  const again = JSON.parse(success(run(binary, ['route', 'key', target, '--to', to, '--json'])));
  assert.equal(again.resolver_key.key_state, 'present', 'a second run finds the key in place');
  assert.equal(again.resolver_key.authorized_keys, 'already_present', 'a second run writes no second line');
  report.cases.push({ case: 'idempotent', receipt: again });

  const authority = first.service_directory_authority?.target;
  assert.ok(authority, 'the receipt names the directory authority');
  const bare = run(binary, ['route', 'key', authority]);
  report.cases.push({ case: 'authority-without-to', refusal: refused(bare,
    /IS the service-directory authority, so its resolver reads the canonical store directly and opens no session to authorize; name the host its adapters reach with --to <host>/,
    'the authority without --to') });
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'route key with --to on the live registry: authorization, idempotence and two refusals; not the resolver tunnel the key then opens (tests/placement/vault_reach.mjs reads over it) or the GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
