import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado placement rehome` on a profile a move carried
// from another account. STADO_REHOME_TEST_PROFILE runs on STADO_REHOME_TEST_HOST
// and its file state STADO_REHOME_TEST_STATE ($HOME/<path>) still names
// STADO_REHOME_TEST_FROM_HOME. The journey: refusals first (a relative
// --from-home, an unknown profile, a host the profile does not run on), then
// the rehome, then the state read back with `service file fetch` names the
// host's own home and no longer the source's, and the replaced file is kept.
// A second rehome finds nothing to rewrite. Required: STADO_BIN,
// STADO_REHOME_TEST_PROFILE, STADO_REHOME_TEST_SERVICE (a service of that
// profile, for the read back), STADO_REHOME_TEST_HOST, STADO_REHOME_TEST_OTHER_HOST,
// STADO_REHOME_TEST_FROM_HOME, STADO_REHOME_TEST_HOME (the host's own home),
// STADO_REHOME_TEST_STATE.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const profile = required('STADO_REHOME_TEST_PROFILE');
const service = required('STADO_REHOME_TEST_SERVICE');
const host = required('STADO_REHOME_TEST_HOST');
const other = required('STADO_REHOME_TEST_OTHER_HOST');
const fromHome = required('STADO_REHOME_TEST_FROM_HOME');
const home = required('STADO_REHOME_TEST_HOME');
const state = required('STADO_REHOME_TEST_STATE');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'placement-rehome-'));
const report = { started_at: new Date().toISOString(), profile, host, from_home: fromHome, state, commands: [], result: 'failed' };
report.stado_sha256 = createHash('sha256').update(readFileSync(stado)).digest('hex');

function run(args) {
  const answer = spawnSync(stado, args, { encoding: 'utf8', timeout: 900_000 });
  const record = { args, status: answer.status, stdout: answer.stdout, stderr_tail: (answer.stderr || '').slice(-3000) };
  report.commands.push(record);
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  return record;
}
const fetchState = name => {
  const destination = join(output, name);
  const fetched = run(['service', 'file', 'fetch', service, '--host', host, '--source-file', state, '--dest-file', destination]);
  assert.equal(fetched.status, 0, fetched.stderr_tail);
  return readFileSync(destination, 'utf8');
};

try {
  const relative = run(['placement', 'rehome', profile, '--host', host, '--from-home', 'Users/someone']);
  assert.notEqual(relative.status, 0, 'a relative --from-home must be refused');
  assert.match(relative.stderr_tail, /not an absolute home directory/);
  const unknown = run(['placement', 'rehome', `${profile}-absent`, '--host', host, '--from-home', fromHome]);
  assert.notEqual(unknown.status, 0, 'an unknown profile must be refused');
  assert.match(unknown.stderr_tail, /holds no placement profile/);
  const elsewhere = run(['placement', 'rehome', profile, '--host', other, '--from-home', fromHome]);
  assert.notEqual(elsewhere.status, 0, 'a host the profile does not run on must be refused');
  assert.match(elsewhere.stderr_tail, new RegExp(`runs on ${host}, not ${other}`));

  const before = fetchState('before');
  assert.ok(before.includes(`${fromHome}/`), `${state} names no ${fromHome} to begin with`);
  const rehomed = run(['placement', 'rehome', profile, '--host', host, '--from-home', fromHome, '--json']);
  assert.equal(rehomed.status, 0, rehomed.stderr_tail);
  const answer = JSON.parse(rehomed.stdout);
  const entry = answer.states.find(each => each.state === state);
  assert.equal(entry?.outcome, 'rewritten', `the state was not rewritten: ${rehomed.stdout}`);
  assert.ok(entry.mentions > 0);
  assert.ok(entry.kept, 'the replaced file is kept');
  const after = fetchState('after');
  assert.ok(!after.includes(`${fromHome}/`), `${state} still names ${fromHome}`);
  assert.ok(after.includes(`${home}/`), `${state} does not name ${home}`);
  assert.equal(after, before.split(`${fromHome}/`).join(`${home}/`), 'only the home changed');

  const again = run(['placement', 'rehome', profile, '--host', host, '--from-home', fromHome, '--json']);
  assert.equal(again.status, 0, again.stderr_tail);
  assert.equal(JSON.parse(again.stdout).states.find(each => each.state === state)?.outcome, 'unchanged');
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`report: ${join(output, 'report.json')} (${report.result})`);
}
