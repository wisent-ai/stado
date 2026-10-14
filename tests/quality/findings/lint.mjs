import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of a refused lint gate's findings. One isolated
// workspace under this repository's `.build`, holding one canonical checkout
// of a one-binary Cargo product whose `.wisent-release.json` declares a
// `format` gate (`cargo fmt --check`) and a `lint` gate (`cargo clippy -- -D
// warnings`), and whose committed source is formatted but carries a clippy
// lint (a needless `return`):
//
// - `stado quality check --revision <commit> --json` refuses the commit and
//   its failure envelope's `details.findings` name src/main.rs:<line> and
//   the clippy lint; the plain check names the same in its refusal;
// - `stado product cargo --revision <commit> --json clippy`, an operation the
//   product's declared gate runs, is admitted and its record's `findings`
//   name the same file:line and lint, so the finding is read on the exact
//   revision without running every gate;
// - `stado product cargo fmt` is refused as a format gate, naming the
//   command that reads it, and an operation no gate declares is refused
//   naming the admitted ones.
//
// Required: STADO_BIN, the Stado under test (a path, or a name on PATH).
// Cargo, clippy and the declared compiler cache are this account's own.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: it names the Stado whose lint findings this test qualifies');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'quality-lint-findings-'));
const workspace = join(output, 'workspace');
const checkout = join(workspace, 'lint-findings-fixture');
const report = { started_at: new Date().toISOString(), workspace, checkout, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = { ...process.env, WISENT_WORKSPACE: workspace };
const MANIFEST = '.wisent-release.json';
const LINT = /^clippy::needless[-_]return$/;
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
const named = finding => finding.location.startsWith('src/main.rs:')
  && typeof finding.lint === 'string' && LINT.test(finding.lint);
let failure;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  // The canonical checkout: on main, identified by a GitHub origin, its
  // gates declared for every platform Stado's own manifest declares, so the
  // one this host runs is among them.
  const own = JSON.parse(readFileSync(join(root, MANIFEST), 'utf8'));
  const gates = [
    { name: 'fmt', argv: ['cargo', 'fmt', '--manifest-path', 'Cargo.toml', '--all', '--', '--check'], kind: 'format' },
    { name: 'clippy', argv: ['cargo', 'clippy', '--manifest-path', 'Cargo.toml', '--locked', '--', '-D', 'warnings'], kind: 'lint' },
  ];
  const platforms = Object.fromEntries(Object.keys(own.platforms).map(platform => [platform, {
    runner_platform: own.platforms[platform].runner_platform,
    quality: gates,
    build: { argv: ['cargo', 'build', '--locked'] },
    stage: { 'Cargo.toml': 'Cargo.toml' },
  }]));
  mkdirSync(join(checkout, 'src'), { recursive: true });
  git(['init', '--quiet', '--initial-branch=main']);
  git(['remote', 'add', 'origin', 'https://github.com/wisent-tests/lint-findings-fixture.git']);
  writeFileSync(join(checkout, '.gitignore'), '/target\n/.build\n/.wisent-output\n');
  writeFileSync(join(checkout, 'Cargo.toml'),
    '[package]\nname = "lint-findings-fixture"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n');
  writeFileSync(join(checkout, 'src/main.rs'),
    'fn answer() -> bool {\n    return true;\n}\n\nfn main() {\n    println!("{}", answer());\n}\n');
  writeFileSync(join(checkout, MANIFEST), `${JSON.stringify({
    schema_version: own.schema_version,
    product: 'lint-findings-fixture',
    releases: true,
    version_source: { kind: 'regex', path: 'Cargo.toml', pattern: '(?m)^version\\s*=\\s*"(?P<version>[^"]+)"\\s*$' },
    platforms,
    promotion: { channels: ['candidate'], reconcile: false },
  }, null, '  ')}\n`);
  success(run('cargo', ['generate-lockfile', '--offline'], checkout));
  git(['add', '.']);
  git(['-c', 'user.name=Stado test', '-c', 'user.email=stado-test@wisent.invalid',
    'commit', '--quiet', '-m', 'a formatted product carrying a clippy lint']);
  const commit = git(['rev-parse', 'HEAD']);
  report.fixture_commit = commit;

  // The quality check: refused, the envelope naming the finding as data.
  const checked = run(binary, ['quality', 'check', '--root', checkout, '--revision', commit, '--json']);
  assert.ok(!succeeded(checked), 'A commit carrying a clippy lint passed its lint gate');
  const envelope = JSON.parse(checked.stdout);
  report.quality_envelope = envelope;
  assert.equal(envelope.status, 'error', 'The refusal is not the JSON failure envelope');
  assert.ok(Array.isArray(envelope.details?.findings) && envelope.details.findings.some(named),
    `The envelope's details do not name src/main.rs:<line> and the needless-return lint: ${checked.stdout}`);
  assert.match(envelope.message, /gate "clippy" \(lint\)/, 'The refusal does not name the lint gate');
  assert.match(envelope.message, /src\/main\.rs:\d+:\d+ \[clippy::needless[-_]return\]/,
    'The refusal sentence does not name the finding');

  const plain = run(binary, ['quality', 'check', '--root', checkout, '--revision', commit]);
  assert.ok(!succeeded(plain), 'The plain check passed the lint');
  assert.match(plain.stderr, /src\/main\.rs:\d+:\d+ \[clippy::needless[-_]return\]/,
    'The plain refusal does not name the finding');

  // The same lint read through product cargo on the same revision.
  const cargo = ['product', 'cargo', '--manifest-path', join(checkout, 'Cargo.toml'), '--revision', commit];
  const linted = run(binary, [...cargo, '--json', 'clippy', '--', '--', '-D', 'warnings']);
  assert.ok(!succeeded(linted), 'product cargo clippy passed a commit carrying a clippy lint');
  const record = JSON.parse(linted.stdout);
  report.cargo_record = record;
  assert.equal(record.operation, 'clippy', 'The record names another operation');
  assert.ok(Array.isArray(record.findings) && record.findings.some(named),
    `product cargo's record does not name src/main.rs:<line> and the lint: ${linted.stdout}`);
  assert.match(record.error, /finding\(s\):\nsrc\/main\.rs:/, 'The cargo failure does not name its findings');

  const formatted = run(binary, [...cargo, 'fmt']);
  assert.ok(!succeeded(formatted), 'product cargo ran a format gate');
  assert.match(formatted.stderr, /is a format gate of .*stado quality check --revision/s,
    'The format refusal does not name the command that reads it');

  const undeclared = run(binary, [...cargo, 'udeps']);
  assert.ok(!succeeded(undeclared), 'product cargo ran an operation no gate declares');
  assert.match(undeclared.stderr, /does not run `cargo udeps`.*\(clippy\)/s,
    'The refusal does not name the operations the tree declares');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'One isolated canonical checkout with a committed clippy lint; quality check (JSON and plain) and product cargo clippy on that revision, a format-gate refusal and an undeclared-operation refusal; GUI parity is the Desktop operations quality-revision and cargo';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
