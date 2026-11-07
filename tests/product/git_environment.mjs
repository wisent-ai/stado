import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification that a product command reads the canonical checkout it
// names, not the repository or index its caller's environment names. A run
// from a git hook carries GIT_DIR, and a session that builds a commit in an
// index of its own exports GIT_INDEX_FILE; git reads those before `-C` or
// the working directory, so a product command that started git with them
// read another repository.
//
// One isolated workspace under this repository's `.build` holds the
// canonical checkout of a one-binary Cargo product and, beside it, a foreign
// repository whose index stages a different source. `stado product cargo
// --revision <commit> build` runs once with GIT_DIR and GIT_INDEX_FILE
// naming the foreign repository and its index:
//
// - the build succeeds and its export marker names the canonical commit;
// - the exported source and the binary built from it say what that commit
//   holds, not what the foreign index stages;
// - the foreign repository's index is unchanged.
//
// Required: STADO_BIN, the Stado under test (a path, or a name on PATH).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: it names the Stado whose git environment this test qualifies');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-git-environment-'));
const workspace = join(output, 'workspace');
const checkout = join(workspace, 'git-environment-fixture');
const foreign = join(output, 'foreign');
const report = { started_at: new Date().toISOString(), workspace, checkout, foreign, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const CANONICAL = 'built from the canonical checkout';
const FOREIGN = 'staged in the foreign index';
const base = { ...process.env, WISENT_WORKSPACE: workspace };
function run(program, args, cwd = root, env = base) {
  const answer = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, cwd, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr,
    git_environment: { GIT_DIR: env.GIT_DIR, GIT_INDEX_FILE: env.GIT_INDEX_FILE } });
  return answer;
}
const succeeded = answer => !answer.error && !answer.signal && !answer.status;
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const commit = (directory, message) => success(run('git', ['-c', 'user.name=Stado test',
  '-c', 'user.email=stado-test@wisent.invalid', 'commit', '--quiet', '-m', message], directory));
function product(directory, said) {
  mkdirSync(join(directory, 'src'), { recursive: true });
  success(run('git', ['init', '--quiet', '--initial-branch=main'], directory));
  writeFileSync(join(directory, '.gitignore'), '/target\n/.build\n/.wisent-output\n');
  writeFileSync(join(directory, 'Cargo.toml'),
    '[package]\nname = "git-environment-fixture"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n');
  writeFileSync(join(directory, 'src/main.rs'), `fn main() {\n    println!("${said}");\n}\n`);
  success(run('cargo', ['generate-lockfile', '--offline'], directory));
  success(run('git', ['add', '.'], directory));
}
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  product(checkout, CANONICAL);
  success(run('git', ['remote', 'add', 'origin', 'https://github.com/wisent-tests/git-environment-fixture.git'], checkout));
  commit(checkout, 'the canonical source');
  const canonical = success(run('git', ['rev-parse', 'HEAD'], checkout));
  report.canonical_commit = canonical;

  // The foreign repository stages another source in its own index.
  product(foreign, FOREIGN);
  const foreignIndex = join(foreign, '.git/index');
  const indexBefore = digest(readFileSync(foreignIndex));

  const bound = { ...base, GIT_DIR: join(foreign, '.git'), GIT_INDEX_FILE: foreignIndex };
  const manifest = join(checkout, 'Cargo.toml');
  const built = run(binary, ['product', 'cargo', '--manifest-path', manifest, '--revision', canonical, '--json', 'build'],
    root, bound);
  success(built);
  report.build = JSON.parse(built.stdout);

  const exported = join(homedir(), '.stado/products/builds', relative(homedir(), checkout), 'stado-build', canonical);
  const marker = JSON.parse(readFileSync(join(exported, '.stado-exported.json'), 'utf8'));
  assert.equal(marker.commit, canonical, 'The export names another commit than the canonical one');
  const source = readFileSync(join(exported, 'src/main.rs'), 'utf8');
  assert.ok(source.includes(CANONICAL) && !source.includes(FOREIGN),
    'The export holds the foreign index, not the canonical commit');
  assert.equal(success(run(join(exported, 'target/debug/git-environment-fixture'), [])), CANONICAL,
    'The binary was not built from the canonical commit');
  assert.equal(digest(readFileSync(foreignIndex)), indexBefore, 'The product command changed the foreign index');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'One canonical checkout and one foreign repository; product cargo --revision build under the foreign GIT_DIR and GIT_INDEX_FILE';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
