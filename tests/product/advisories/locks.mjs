import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of `stado product advisories list|apply`: the Stado under test
// reads, against the public OSV database, the committed Cargo.lock files of a
// workspace of two git checkouts under .build. `example-app` locks
// webpki-roots 1.0.9, which RUSTSEC-2026-0381 names and 1.0.10 fixes within
// its caret range; `example-legacy` pins rustls-webpki =0.101.7 in its
// manifest, which RUSTSEC-2026-0382 names and no compatible version fixes.
// The journey reads each refusal, the findings, an apply refused on an
// uncommitted lockfile and left unchanged, an apply that rewrites the lock
// on disk and commits nothing, and a package whose fix needs a manifest
// change named as unfixed. Required: STADO_BIN, the built Stado under test;
// Cargo on PATH, whose own homes the Stado process keeps.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN names the built Stado under test');
const operatorHome = process.env.HOME;
assert.ok(operatorHome, 'HOME names where Cargo and rustup keep their own homes');
mkdirSync(join(root, '.build'), { recursive: true });
const area = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-advisories-'));
const home = join(area, 'home');
const workspace = join(area, 'workspace');
mkdirSync(join(home, 'tmp'), { recursive: true });

const toolchain = {
  PATH: process.env.PATH,
  CARGO_HOME: join(operatorHome, '.cargo'),
  RUSTUP_HOME: join(operatorHome, '.rustup'),
};
function run(program, args, cwd, env = toolchain) {
  const answer = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  assert.ok(!answer.error && !answer.signal, `${program} ${args.join(' ')}: ${answer.error?.message}`);
  return answer;
}
function checked(program, args, cwd) {
  const answer = run(program, args, cwd);
  assert.equal(answer.status, 0, `${program} ${args.join(' ')}: ${answer.stderr}`);
  return answer.stdout.trim();
}
const git = (cwd, ...args) => checked('git', ['-c', 'user.name=example', '-c', 'user.email=example@example.invalid', ...args], cwd);

// A checkout whose committed lock resolves `requirement` of `crate` at `locked`.
function checkout(name, crate, requirement, locked) {
  const path = join(workspace, name);
  mkdirSync(join(path, 'src'), { recursive: true });
  writeFileSync(join(path, 'Cargo.toml'), `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n${crate} = "${requirement}"\n`);
  writeFileSync(join(path, 'src', 'lib.rs'), '');
  checked('cargo', ['generate-lockfile'], path);
  checked('cargo', ['update', '--package', crate, '--precise', locked], path);
  git(path, 'init', '--quiet');
  git(path, 'add', 'Cargo.toml', 'Cargo.lock', 'src/lib.rs');
  git(path, 'commit', '--quiet', '-m', 'example');
  return path;
}
const app = checkout('example-app', 'webpki-roots', '1', '1.0.9');
const legacy = checkout('example-legacy', 'rustls-webpki', '=0.101.7', '0.101.7');
const locked = (path, crate) => readFileSync(join(path, 'Cargo.lock'), 'utf8').match(new RegExp(`name = "${crate}"\\nversion = "([^"]+)"`))[1];

const env = { ...toolchain, HOME: home, STADO_CONFIG: join(home, 'no-config.toml'), TMPDIR: join(home, 'tmp') };
const runs = [];
function advisories(...args) {
  const answer = run(stado, ['product', '--workspace', workspace, 'advisories', ...args], root, env);
  runs.push({ args, exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr });
  writeFileSync(join(area, 'report.json'), JSON.stringify(runs, null, 2));
  return answer;
}
function refused(answer, sentence) {
  assert.notEqual(answer.status, 0, answer.stdout);
  assert.ok(answer.stderr.includes(sentence), `expected ${JSON.stringify(sentence)} in: ${answer.stderr}`);
}

// Refusals before anything is read.
refused(advisories('list', 'example-missing'), 'names no product example-missing');
refused(advisories('list', '--database', 'not a url'), '--database "not a url" is not a URL; give an https origin');
refused(
  advisories('list', '--advisory', 'RUSTSEC-0000-0000'),
  'holds no crates.io advisory RUSTSEC-0000-0000; name an id or alias it publishes',
);

// list: both locks are read and each affected version is named.
const listed = advisories('list', '--advisory', 'RUSTSEC-2026-0381', '--advisory', 'RUSTSEC-2026-0382', '--json');
assert.equal(listed.status, 1, listed.stderr);
const before = JSON.parse(listed.stdout);
assert.deepEqual(before.checkouts.map((entry) => [entry.checkout, entry.read, entry.lockfiles]), [
  [app, true, ['Cargo.lock']],
  [legacy, true, ['Cargo.lock']],
]);
const roots = before.findings.find((finding) => finding.package === 'webpki-roots');
assert.equal(roots.version, '1.0.9');
assert.equal(roots.advisory, 'RUSTSEC-2026-0381');
assert.equal(roots.compatible_fix, '1.0.10');
const verifier = before.findings.find((finding) => finding.package === 'rustls-webpki');
assert.equal(verifier.version, '0.101.7');
assert.equal(verifier.compatible_fix, null);
assert.deepEqual(verifier.dependents, ['example-legacy 0.1.0']);

// apply on an uncommitted lockfile is refused and leaves it as it was.
appendFileSync(join(app, 'Cargo.lock'), '\n');
const dirty = advisories('apply', '--advisory', 'RUSTSEC-2026-0381', '--json');
assert.equal(dirty.status, 1, dirty.stderr);
const [held] = JSON.parse(dirty.stdout).updates;
assert.equal(held.state, 'refused');
assert.ok(held.cause.includes('has uncommitted changes'), held.cause);
assert.equal(locked(app, 'webpki-roots'), '1.0.9');
git(app, 'checkout', '--', 'Cargo.lock');

// apply locks the compatible fix on disk, names the unfixed one, commits nothing.
const head = git(app, 'rev-parse', 'HEAD');
const applied = advisories('apply', '--advisory', 'RUSTSEC-2026-0381', '--advisory', 'RUSTSEC-2026-0382', '--json');
assert.equal(applied.status, 1, applied.stderr);
const after = JSON.parse(applied.stdout);
const states = Object.fromEntries(after.updates.map((update) => [update.package, update]));
assert.equal(states['webpki-roots'].state, 'updated');
assert.equal(states['webpki-roots'].to, '1.0.10');
assert.equal(states['rustls-webpki'].state, 'unfixed');
assert.ok(
  states['rustls-webpki'].cause.includes('no fixed version of rustls-webpki is caret-compatible with 0.101.7')
    && states['rustls-webpki'].cause.includes('the requirement of example-legacy 0.1.0 on it must move'),
  states['rustls-webpki'].cause,
);
assert.equal(locked(app, 'webpki-roots'), '1.0.10');
assert.equal(git(app, 'rev-parse', 'HEAD'), head);
assert.equal(git(app, 'status', '--porcelain'), 'M Cargo.lock');
assert.deepEqual(after.findings.map((finding) => finding.package), ['rustls-webpki']);

// An advisory is named by its id or any alias: RUSTSEC-2026-0381 lists
// RUSTSEC-2026-0382 among its aliases, so the legacy lock still stands. Once
// the fix is committed and the workspace holds only the fixed checkout, list
// settles.
git(app, 'commit', '--quiet', '-am', 'lock the fix');
const standing = advisories('list', '--advisory', 'RUSTSEC-2026-0381', '--json');
assert.equal(standing.status, 1, standing.stderr);
assert.deepEqual(JSON.parse(standing.stdout).findings.map((finding) => finding.checkout), [legacy]);
rmSync(legacy, { recursive: true });
const settled = advisories('list', '--advisory', 'RUSTSEC-2026-0381', '--json');
assert.equal(settled.status, 0, settled.stderr);
assert.deepEqual(JSON.parse(settled.stdout).findings, []);

// The workspace and home go; the report of every run stays as the evidence.
rmSync(workspace, { recursive: true });
rmSync(home, { recursive: true });
console.log(join(area, 'report.json'));
