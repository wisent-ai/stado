import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado product cargo --revision <commit>`: a build of
// one committed revision of a checkout while that checkout's working tree
// holds an uncommitted edit that does not compile, as it does while another
// session is editing it. One isolated workspace under this repository's
// `.build`, holding one canonical checkout of a one-binary Cargo product:
//
// - the build of the committed revision succeeds, reads the tree exported
//   under `<checkout>/.build/stado-build/<commit>/`, and the binary it made
//   prints what the commit holds, not what the working tree holds;
// - the same build without `--revision` reads the working tree and fails on
//   the uncommitted edit, so the success above is the revision's doing;
// - a commit the checkout does not hold is refused by name and exports
//   nothing;
// - the uncommitted edit is still in the working tree afterwards.
//
// Required: STADO_BIN, the Stado under test (a path, or a name on PATH).
// Cargo and the declared compiler cache are the real ones of this account.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: it names the Stado whose product cargo --revision this test qualifies');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-cargo-revision-'));
const workspace = join(output, 'workspace');
const checkout = join(workspace, 'cargo-revision-fixture');
const report = { started_at: new Date().toISOString(), workspace, checkout, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const COMMITTED = 'built from the committed revision';
const environment = { ...process.env, WISENT_WORKSPACE: workspace };
function run(program, args, cwd = root) {
  const answer = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, cwd, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const succeeded = answer => !answer.error && !answer.signal && !answer.status;
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const git = args => success(run('git', args, checkout));
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.source_diff = success(run('git', ['diff', 'HEAD', '--', 'stado-rs/product/src/cargo', 'stado-rs/product/src/cli', 'tests/product']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  // The canonical checkout: on main, identified by a GitHub origin, with a
  // locked one-binary product committed.
  mkdirSync(join(checkout, 'src'), { recursive: true });
  git(['init', '--quiet', '--initial-branch=main']);
  git(['remote', 'add', 'origin', 'https://github.com/wisent-tests/cargo-revision-fixture.git']);
  writeFileSync(join(checkout, '.gitignore'), '/target\n/.build\n/.wisent-output\n');
  writeFileSync(join(checkout, 'Cargo.toml'),
    '[package]\nname = "cargo-revision-fixture"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n');
  writeFileSync(join(checkout, 'src/main.rs'), `fn main() {\n    println!("${COMMITTED}");\n}\n`);
  success(run('cargo', ['generate-lockfile', '--offline'], checkout));
  git(['add', '.']);
  git(['-c', 'user.name=Stado test', '-c', 'user.email=stado-test@wisent.invalid',
    'commit', '--quiet', '-m', 'a committed one-binary product']);
  const commit = git(['rev-parse', 'HEAD']);
  report.fixture_commit = commit;

  // Another session's uncommitted edit: the working tree no longer compiles.
  const edit = 'fn main() {\n    this edit is half written\n}\n';
  writeFileSync(join(checkout, 'src/main.rs'), edit);
  const manifest = join(checkout, 'Cargo.toml');
  const cargo = ['product', 'cargo', '--manifest-path', manifest];

  const working = run(binary, [...cargo, 'build']);
  assert.ok(!succeeded(working), 'The working tree holds an edit that does not compile, yet its build passed');

  const absent = createHash('sha1').update('a commit this checkout never held').digest('hex');
  const refused = run(binary, [...cargo, '--revision', absent, 'build']);
  assert.ok(!succeeded(refused), 'A commit the checkout does not hold was built');
  assert.match(refused.stderr, new RegExp(`does not hold commit ${absent}`), 'The refusal must name the commit');
  const exportedRoot = join(checkout, '.build/stado-build');
  assert.ok(!existsSync(join(exportedRoot, absent)), 'A refused revision left an export behind');

  const built = run(binary, [...cargo, '--revision', commit, '--json', 'build']);
  success(built);
  report.revision_build = JSON.parse(built.stdout);
  const exported = join(exportedRoot, commit);
  const marker = JSON.parse(readFileSync(join(exported, '.stado-exported.json'), 'utf8'));
  assert.equal(marker.commit, commit, 'The export marker names another commit');
  assert.ok(readFileSync(join(exported, 'src/main.rs'), 'utf8').includes(COMMITTED),
    'The export does not hold the committed source');
  const said = success(run(join(exported, 'target/debug/cargo-revision-fixture'), []));
  assert.equal(said, COMMITTED, 'The binary was not built from the committed revision');

  assert.equal(readFileSync(join(checkout, 'src/main.rs'), 'utf8'), edit,
    'The build changed the uncommitted edit in the working tree');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'One isolated canonical checkout with an uncommitted edit; product cargo build with and without --revision and one refusal; not stage, install or GUI';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
