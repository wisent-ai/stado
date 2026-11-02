import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Qualification of `stado host local vault-reach`, the step a vault move
// runs on its source before the commit. From this machine, against a real
// registered host serving the fleet vault: every object API bearer's
// revision is read as consumer `stado` over the native SSH channel the
// resolver opens, the session within the resolver's refresh interval and
// each read within that interval less the session's time, and the receipt
// says `reached: true` with one answered read per namespace. Refusals: an
// endpoint that is not a URL, and an endpoint on that host where nothing
// listens, which must end `reached: false`, exit non-zero and name the cause.
// Required: STADO_BIN, the Stado under test; STADO_VAULT_REACH_TEST_HOST, the
// registered host; STADO_VAULT_REACH_TEST_ENDPOINT, the vault endpoint the
// service directory declares there; STADO_VAULT_REACH_TEST_CLOSED_ENDPOINT,
// an endpoint on that host where nothing listens.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const build = realpathSync(join(root, '.build'));
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const host = required('STADO_VAULT_REACH_TEST_HOST');
const endpoint = required('STADO_VAULT_REACH_TEST_ENDPOINT');
const closed = required('STADO_VAULT_REACH_TEST_CLOSED_ENDPOINT');
const output = mkdtempSync(join(build, 'placement-vault-reach-'));
const report = { started_at: new Date().toISOString(), host, endpoint, commands: [], result: 'failed' };
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
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };
  const reach = at => run(binary, ['host', 'local', 'vault-reach', '--host', host, '--endpoint', at]);

  const notUrl = reach('no-url');
  assert.ok(notUrl.status !== null && notUrl.status, 'an endpoint that is not a URL must be refused');
  assert.match(notUrl.stderr, /is not a URL/);

  const live = JSON.parse(success(reach(endpoint)));
  report.live = live;
  assert.equal(live.schema, 'stado.vault-reach.v1');
  assert.equal(live.reached, true, `the live vault was not read: ${live.cause}`);
  assert.ok(live.bound_seconds > 0, 'the bound comes from a declared refresh interval');
  assert.ok(live.reads.length > 0, 'every object API namespace bearer is read');
  assert.ok(live.paths.length > 0, 'the receipt names the SSH paths it tried');
  assert.ok(
    live.paths_seconds <= live.session_seconds && live.session_seconds <= live.seconds,
    'the path choice, then the session, then the reads are timed in order',
  );
  assert.ok(
    Math.abs(live.read_bound_seconds - (live.bound_seconds - live.session_seconds)) < 0.001,
    'each read is bounded by the refresh interval less the session time, not all reads together',
  );
  for (const read of live.reads) {
    assert.equal(read.answered, true, `${read.namespace} (${read.item}) answered ${JSON.stringify(read.answer)}`);
    assert.ok(read.seconds < live.read_bound_seconds, `${read.namespace} answered past its bound`);
  }

  const dead = reach(closed);
  assert.ok(dead.status !== null && dead.status, 'an endpoint where nothing listens must exit non-zero');
  const deadReceipt = JSON.parse(dead.stdout.trim());
  report.closed = deadReceipt;
  assert.equal(deadReceipt.reached, false);
  assert.ok(deadReceipt.cause, 'the refusal names its cause');
  assert.equal(
    typeof deadReceipt.session_seconds,
    'number',
    'a closed endpoint on a reachable host still opens the session, so the cause is the read',
  );
  assert.match(dead.stderr, /was not read from this host/);

  report.result = 'passed';
} catch (error) {
  failure = error;
  report.error = error.stack ?? String(error);
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(join(output, 'report.json'));
}
if (failure) throw failure;
