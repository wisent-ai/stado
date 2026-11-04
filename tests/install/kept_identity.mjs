import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of a recipe that declares `signing_role` beside
// `signing_keychain` on a Mac whose own keychains do not hold that role's
// identity. The Stado under test installs STADO_KEPT_TEST_PRODUCT's cli surface
// from STADO_KEPT_TEST_COMMIT with no signing flag and no signing variable.
// It must keep the role's identity in the recipe's keychain under
// ~/.stado/signing (`signing: role <role>'s identity <sha1> is kept|was
// imported in <keychain>`), sign every placed native file with that identity,
// record `signing.kept_identity` in the receipt, and make no identity change,
// where before it refused because the operator's login keychain does not hold
// the fleet identity. Required: STADO_BIN, STADO_KEPT_TEST_PRODUCT,
// STADO_KEPT_TEST_COMMIT.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const stado = required('STADO_BIN');
const product = required('STADO_KEPT_TEST_PRODUCT');
const commit = required('STADO_KEPT_TEST_COMMIT');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'install-kept-identity-'));
const report = { started_at: new Date().toISOString(), product, commit, commands: [], result: 'failed' };
report.stado_sha256 = createHash('sha256').update(readFileSync(stado)).digest('hex');
const env = { ...process.env };
for (const name of ['WISENT_CODESIGN_KEYCHAIN', 'WISENT_CODESIGN_IDENTITY', 'WISENT_CODESIGN_ROLE',
  'WISENT_CODESIGN_CERTIFICATE_PEM', 'WISENT_CODESIGN_PRIVATE_KEY_PEM', 'MACOS_SIGN_IDENTITY']) delete env[name];
const save = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, 2));

function run(args) {
  const answer = spawnSync(stado, args, { encoding: 'utf8', env, timeout: 3_300_000 });
  // The kept-identity line is said before the build, ahead of thousands of
  // build lines, so it is read from the whole stderr; the report keeps a tail.
  const record = { args, status: answer.status, signal: answer.signal, stdout: answer.stdout,
    stderr_tail: (answer.stderr || '').slice(-6000) };
  report.commands.push(record);
  save();
  return { ...record, stderr: answer.stderr || '' };
}

try {
  const installed = run(['product', 'install', product, '--surface', 'cli', '--source-commit', commit, '--wait', '--json']);
  assert.equal(installed.status, 0, installed.stderr_tail);
  const said = /signing: role (\S+)'s identity ([0-9A-F]{40}) (is kept|was imported) in (\S+)/.exec(installed.stderr);
  assert.ok(said, 'the installation did not say which role identity it kept');
  const [, role, sha1, , keychain] = said;
  assert.match(keychain, /\/\.stado\/signing\/[^/]+\.keychain-db$/, `kept outside Stado's signing directory: ${keychain}`);
  assert.doesNotMatch(installed.stderr, /signing identity .* -> /, 'the installation changed the installed identity');
  const status = run(['product', 'status', product, '--surface', 'cli', '--json']);
  assert.equal(status.status, 0, status.stderr_tail);
  const signing = JSON.parse(status.stdout).signing;
  report.signing = signing;
  assert.equal(signing?.keychain, keychain);
  assert.deepEqual(signing?.kept_identity, { role, outcome: signing.kept_identity.outcome, identity: sha1 });
  assert.ok(['kept', 'imported'].includes(signing.kept_identity.outcome));
  const signatures = Object.values(signing.signatures ?? {});
  assert.ok(signatures.length > 0, 'the receipt names no signed native file');
  const listed = spawnSync('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning', keychain], { encoding: 'utf8' });
  const authority = new RegExp(`${sha1} "([^"]+)"`).exec(listed.stdout)?.[1];
  assert.ok(authority, `${keychain} does not list ${sha1}: ${listed.stdout}`);
  for (const signature of signatures) {
    assert.equal(signature.identity, authority, `a placed file is signed by ${signature.identity}, not ${authority}`);
  }
  // The kept keychain is listed only while Stado signs: left on the user's
  // search list, every other program asking it for an identity met the
  // fleet's key, locked after a restart, behind a password dialog.
  const searched = spawnSync('/usr/bin/security', ['list-keychains', '-d', 'user'], { encoding: 'utf8' });
  report.search_list = searched.stdout;
  assert.equal(searched.status, 0, searched.stderr);
  assert.ok(!searched.stdout.includes(keychain), `${keychain} is still on the user's search list: ${searched.stdout}`);
  report.result = 'passed';
} finally {
  report.finished_at = new Date().toISOString();
  save();
  console.log(`report: ${join(output, 'report.json')} (${report.result})`);
}
