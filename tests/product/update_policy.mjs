import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of a host updating its products by itself, through the
// update policy `stado product sync` follows. The Stado under test (STADO_BIN)
// runs with HOME set to an isolated directory below this checkout's .build,
// against a catalog of one product, `fixture-tool`, whose GitHub origin is a
// local bare repository (git's url.insteadOf, in a git configuration of the
// journey's own). Each commit pushed to that origin is what a landed commit
// is to a real host. The journey proves, through the real CLI:
// - a sync pass installs origin/main with nobody naming a revision, and the
//   installed program answers with the new version;
// - a lock record a crashed or older writer left unreadable (zero bytes in
//   front of its JSON, as one host's sync.lock was) does not stop the next pass;
// - a product the policy holds is not updated, and its row says why and how
//   to release it; one outside its window is deferred; status shows the rule;
// - an update whose `health` command fails is rolled back to the installation
//   before it, the failed revision is not installed again on the next pass,
//   and the next commit is;
// - a policy file that cannot be followed refuses the pass with exit 1 and a
//   sentence naming the file and what to write.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-update-policy-'));
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN names the Stado under test');
const report = { started_at: new Date().toISOString(), commands: [], steps: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const home = join(output, 'home');
const workspace = join(output, 'workspace');
const origin = join(output, 'origin', 'fixture-tool.git');
const authoring = join(output, 'authoring');
const catalogRepository = join(output, 'catalog');
const gitConfig = join(output, 'gitconfig');
for (const directory of [home, workspace, dirname(origin), authoring, catalogRepository]) {
  mkdirSync(directory, { recursive: true });
}
writeFileSync(gitConfig, [
  '[user]', '\tname = journey', '\temail = journey@example.invalid',
  `[url "file://${join(output, 'origin')}/"]`, '\tinsteadOf = https://github.com/wisent-ai/',
  '[init]', '\tdefaultBranch = main', '[advice]', '\tdetachedHead = false', '',
].join('\n'));
const environment = {
  ...process.env, HOME: home, GIT_CONFIG_GLOBAL: gitConfig, GIT_CONFIG_NOSYSTEM: '1',
  WISENT_WORKSPACE: workspace, PATH: `${join(home, '.local/bin')}:${process.env.PATH}`,
};
function run(program, args, cwd = output) {
  const answer = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, cwd, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.equal(answer.status, 0, `${answer.stderr}\n${answer.stdout}` || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const product = (...args) => run(stado, ['product', '--workspace', workspace,
  '--catalog', join(catalogRepository, 'catalog/products.yml'), ...args]);
function sync() {
  const answer = product('sync', '--surface', 'cli', '--fetch', '--json');
  const rows = JSON.parse(answer.stdout);
  const row = rows.find(entry => entry.product === 'fixture-tool');
  assert.ok(row, `the sync decides fixture-tool: ${answer.stdout}\n${answer.stderr}`);
  return { answer, row };
}
const installed = join(home, '.local/bin/fixture-tool');
const version = () => success(run(installed, []));
function commit(label, healthy) {
  writeFileSync(join(authoring, 'fixture-tool'), [
    '#!/bin/sh',
    `if [ "$1" = "--health" ]; then exit ${healthy ? 0 : 1}; fi`,
    `echo ${label}`, '',
  ].join('\n'));
  chmodSync(join(authoring, 'fixture-tool'), 0o755);
  success(run('git', ['add', '-A'], authoring));
  success(run('git', ['commit', '-q', '-m', label], authoring));
  success(run('git', ['push', '-q', 'origin', 'HEAD:main'], authoring));
  return success(run('git', ['rev-parse', 'HEAD'], authoring));
}
const step = (name, detail) => report.steps.push({ name, ...detail });
try {
  // An export of a commit (`git archive`) is not a checkout: its runner names
  // the commit in STADO_TEST_REVISION; a checkout answers for itself.
  report.source_revision = process.env.STADO_TEST_REVISION
    || success(run('git', ['rev-parse', 'HEAD'], root));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = resolve(stado);
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  // The product's origin and its first commit.
  success(run('git', ['init', '-q', '--bare', origin]));
  success(run('git', ['init', '-q'], authoring));
  success(run('git', ['remote', 'add', 'origin', 'https://github.com/wisent-ai/fixture-tool'], authoring));
  writeFileSync(join(authoring, 'install.sh'),
    '#!/bin/sh\nset -e\nmkdir -p "$HOME/.local/bin"\ncp fixture-tool "$HOME/.local/bin/fixture-tool"\n');
  writeFileSync(join(authoring, 'install-user.sh'),
    '#!/bin/sh\nset -e\nmkdir -p "$HOME/.local/bin"\nprintf "#!/bin/sh\\necho user\\n" > "$HOME/.local/bin/fixture-user"\nchmod +x "$HOME/.local/bin/fixture-user"\n');
  const first = commit('v1', true);
  // The workspace's checkout of it, as `--clone-missing` would make.
  success(run('git', ['clone', '-q', 'https://github.com/wisent-ai/fixture-tool', join(workspace, 'fixture-tool')]));

  // The catalog: one product, installed by its own script, checked by its
  // own program after every update.
  mkdirSync(join(catalogRepository, 'catalog'));
  writeFileSync(join(catalogRepository, 'catalog/products.yml'), [
    'products:', '- id: fixture-tool', '  name: Fixture tool', '  status: active',
    '  visibility: private', '  family: standalone', '  owner_repository: wisent-ai/fixture-tool',
    '  description: The product a self-updating host keeps current in this journey.',
    '  roadmap: []', '  integrations: []', '  evidence:', '  - wisent-ai/fixture-tool/install.sh', '  surfaces:',
    '  - kind: cli', '    repository: wisent-ai/fixture-tool', '  installations:',
    '  - surface: cli', '    kind: local-build', '    repository: wisent-ai/fixture-tool',
    '    command: sh install.sh', '    binaries:', '    - fixture-tool',
    '    health:', '    - fixture-tool', '    - --health',
    // A second product that cannot run without fixture-tool: installing it
    // installs fixture-tool as its dependency, unless the policy holds it.
    '- id: fixture-user', '  name: Fixture user', '  status: active',
    '  visibility: private', '  family: standalone', '  owner_repository: wisent-ai/fixture-tool',
    '  description: A product bound to fixture-tool, installed by hand in this journey.',
    '  roadmap: []', '  integrations: []', '  evidence:', '  - wisent-ai/fixture-tool/install-user.sh', '  surfaces:',
    '  - kind: cli', '    repository: wisent-ai/fixture-tool', '  installations:',
    '  - surface: cli', '    kind: local-build', '    repository: wisent-ai/fixture-tool',
    '    command: sh install-user.sh', '    binaries:', '    - fixture-user',
    '  dependencies:', '  - product: fixture-tool', '    surface: cli', '    for_surface: cli',
    '    bound_because: the journey binds it so a held dependency must refuse its dependent', '',
  ].join('\n'));
  success(run('git', ['init', '-q'], catalogRepository));
  success(run('git', ['add', '-A'], catalogRepository));
  success(run('git', ['commit', '-q', '-m', 'catalog'], catalogRepository));

  // 1. No policy: the default installs what origin/main carries.
  let { answer, row } = sync();
  step('default policy installs', { exit: answer.status, row });
  assert.equal(answer.status, 0, answer.stderr);
  assert.equal(row.decision, 'installed', row.detail);
  assert.equal(row.source_revision, first);
  assert.equal(row.policy.mode, 'automatic');
  assert.equal(row.policy.from, 'default');
  assert.equal(version(), 'v1');

  // 2. A lock record nobody can read does not stop the next pass.
  const lock = join(home, '.stado/products/sync.lock');
  writeFileSync(lock, Buffer.concat([Buffer.alloc(468), Buffer.from('{"pid":1,"surface":"cli"}')]));
  const second = commit('v2', true);
  ({ answer, row } = sync());
  step('unreadable lock record reclaimed', { exit: answer.status, row, stderr: answer.stderr.split('\n').filter(line => line.includes('sync.lock')) });
  assert.equal(answer.status, 0, answer.stderr);
  assert.match(answer.stderr, /reclaiming released lock .*sync\.lock: its holder record cannot be read/);
  assert.equal(row.decision, 'installed', row.detail);
  assert.equal(version(), 'v2');
  assert.doesNotThrow(() => JSON.parse(readFileSync(lock, 'utf8')), 'the lock holds one readable record');

  // 3. Held: a newer commit is not installed, and the row says why.
  success(product('policy', 'set', 'fixture-tool', '--mode', 'held', '--reason', 'journey holds it'));
  const third = commit('v3', true);
  ({ answer, row } = sync());
  step('held product not updated', { exit: answer.status, row });
  assert.equal(answer.status, 0, answer.stderr);
  assert.equal(row.decision, 'held');
  assert.match(row.detail, /holds fixture-tool: journey holds it; release it with `stado product policy set fixture-tool --mode automatic`/);
  assert.equal(row.policy.from, 'product');
  assert.equal(version(), 'v2');
  const status = JSON.parse(success(product('status', 'fixture-tool', '--surface', 'cli', '--json')));
  step('status shows the rule', { update_policy: status.update_policy });
  assert.equal(status.update_policy.mode, 'held');

  // 4. A window that is closed now defers the update.
  const hour = new Date().getHours();
  const closed = `${String((hour + 2) % 24).padStart(2, '0')}:00-${String((hour + 3) % 24).padStart(2, '0')}:00`;
  success(product('policy', 'set', 'fixture-tool', '--mode', 'automatic', '--window', closed));
  ({ answer, row } = sync());
  step('closed window defers', { exit: answer.status, row, window: closed });
  assert.equal(row.decision, 'deferred');
  assert.match(row.detail, new RegExp(`only between ${closed} local time`));
  assert.equal(version(), 'v2');

  // 5. Automatic again: the held commit is installed.
  success(product('policy', 'remove', 'fixture-tool'));
  ({ answer, row } = sync());
  step('released product updated', { exit: answer.status, row });
  assert.equal(row.decision, 'installed', row.detail);
  assert.equal(row.source_revision, third);
  assert.equal(version(), 'v3');

  // 5b. A held product is never installed, repaired or rebuilt as another
  // product's dependency. Its receipt says installed but its program was
  // deleted: the dependent uses it as it is and leaves it gone.
  success(product('policy', 'set', 'fixture-tool', '--mode', 'held', '--reason', 'journey holds it'));
  const program = readFileSync(installed);
  rmSync(installed);
  answer = product('install', 'fixture-user', '--surface', 'cli', '--json');
  step('held installed dependency used as it is', { exit: answer.status,
    stderr: answer.stderr.split('\n').filter(line => line.includes('fixture-tool/cli')) });
  assert.equal(answer.status, 0, answer.stderr);
  assert.match(answer.stderr, /fixture-user: uses fixture-tool\/cli as it is installed, without checking or rebuilding it: the update policy .* holds fixture-tool: journey holds it/);
  assert.equal(existsSync(installed), false, 'the held dependency was rebuilt for its dependent');
  // Removed: the dependent bound to it is refused, and it stays removed. Its
  // program is put back first so the removal verifies what it deletes.
  writeFileSync(installed, program, { mode: 0o755 });
  success(product('remove', 'fixture-tool', '--surface', 'cli', '--json'));
  answer = product('install', 'fixture-user', '--surface', 'cli', '--json');
  step('held dependency not installed for a dependent', { exit: answer.status,
    stderr: answer.stderr.split('\n').filter(line => line.startsWith('Error:')) });
  assert.notEqual(answer.status, 0, 'the dependent bound to a held product must be refused');
  assert.match(answer.stderr, /fixture-user\/cli needs fixture-tool\/cli, and the update policy .* holds fixture-tool: journey holds it/);
  const removed = JSON.parse(success(product('status', 'fixture-tool', '--surface', 'cli', '--json')));
  assert.equal(removed.status, 'absent', 'the held dependency stays removed');
  // The operator releases it and installs it again by hand.
  success(product('policy', 'remove', 'fixture-tool'));
  success(product('install', 'fixture-tool', '--surface', 'cli', '--json'));
  assert.equal(version(), 'v3');

  // 6. A commit whose health command fails is rolled back, and not retried:
  // not by the tool's own row, and not as a dependency of fixture-user, whose
  // row the same pass installs after it. The predecessor the rollback put
  // back is a whole installation the dependent uses.
  const broken = commit('v4', false);
  ({ answer, row } = sync());
  step('failed health rolled back', { exit: answer.status, row });
  assert.equal(answer.status, 0, answer.stderr);
  assert.equal(row.decision, 'rolled-back', row.detail);
  assert.match(row.detail, new RegExp(`fixture-tool ${broken} did not pass its readiness check: its health command \`fixture-tool --health\` exited`));
  assert.equal(row.source_revision, third);
  assert.doesNotMatch(answer.stderr, /fixture-tool\/cli: "build": "dependency installation is not complete"/,
    'the dependent rebuilt the rolled-back dependency');
  assert.equal(version(), 'v3');
  ({ answer, row } = sync());
  step('rolled-back revision not reinstalled', { exit: answer.status, row });
  assert.equal(row.decision, 'held');
  assert.match(row.detail, new RegExp(`origin/main is still ${broken}, which product sync installed and rolled back`));
  assert.equal(version(), 'v3');
  const repaired = commit('v5', true);
  ({ answer, row } = sync());
  step('next commit installed after rollback', { exit: answer.status, row });
  assert.equal(row.decision, 'installed', row.detail);
  assert.equal(row.source_revision, repaired);
  assert.equal(version(), 'v5');

  // 7. Refusals: a policy that cannot be followed, and a held rule with no reason.
  const policyFile = join(home, '.stado/products/update-policy.json');
  writeFileSync(policyFile, '{"products":{"fixture-tool":{"mode":"sometimes"}}}\n');
  answer = product('sync', '--surface', 'cli', '--json');
  step('unfollowable policy refused', { exit: answer.status, stderr: answer.stderr });
  assert.equal(answer.status, 1);
  assert.match(answer.stderr, /products\.fixture-tool: mode "sometimes" is not an update mode, so the policy cannot say whether the product installs by itself; write "automatic" or "held"/);
  rmSync(policyFile);
  answer = product('policy', 'set', 'fixture-tool', '--mode', 'held');
  step('held without reason refused', { exit: answer.status, stderr: answer.stderr });
  assert.equal(answer.status, 1);
  assert.match(answer.stderr, /holds the product without a reason, so nobody can tell when it may be released; add "reason"/);
  assert.match(answer.stderr, /\[config\]/);
  report.revisions = { first, second, third, broken, repaired };
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'stado product sync and stado product policy against a local-build product whose origin is a local bare repository, in an isolated HOME';
  for (const name of ['home', 'workspace', 'origin', 'authoring', 'catalog']) {
    rmSync(join(output, name), { recursive: true, force: true });
  }
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
