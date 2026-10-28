import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of `stado product advisories list|apply`: the Stado under test
// reads, against the public OSV database, the committed Cargo.lock files of a
// workspace of git checkouts under .build. `example-app` locks webpki-roots
// 1.0.9, which RUSTSEC-2026-0381 names and 1.0.10 fixes within its caret
// range; `example-legacy` pins rustls-webpki =0.101.7 in its manifest, which
// RUSTSEC-2026-0382 names and no compatible version fixes; `example-verifier`
// locks rustls-webpki 0.103.13 with aws-lc-rs 1.17.3, and the fixed 0.103.16
// requires a newer aws-lc-rs. The journey reads each refusal (a withdrawn
// advisory among them), the findings, the same findings from a run that
// inherits another repository's GIT_DIR and index, an apply refused on an uncommitted
// lockfile and left unchanged, an apply that rewrites a lock on disk and
// commits nothing while the committed findings stand, a list that keeps
// reading the committed lock until the fix is committed, the package Cargo
// moved beside the fix, and a package whose fix needs a manifest change
// named as unfixed. A run with a finding standing exits with a failure
// status, and a settled one successfully. Required: STADO_BIN, the built
// Stado under test; Cargo on PATH, whose own homes the Stado process keeps.
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
// Whether a process that ran to its end (run() refuses a signal) exited
// with a failure status.
const failed = (answer) => Boolean(answer.status);
function checked(program, args, cwd) {
  const answer = run(program, args, cwd);
  assert.ok(!failed(answer), `${program} ${args.join(' ')}: ${answer.stderr}`);
  return answer.stdout.trim();
}
const git = (cwd, ...args) => checked('git', ['-c', 'user.name=example', '-c', 'user.email=example@example.invalid', ...args], cwd);

// A checkout whose manifest carries `dependency` and whose committed lock
// holds each [crate, version] of `pins`.
function checkout(name, dependency, pins) {
  const path = join(workspace, name);
  mkdirSync(join(path, 'src'), { recursive: true });
  writeFileSync(join(path, 'Cargo.toml'), `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n${dependency}\n`);
  writeFileSync(join(path, 'src', 'lib.rs'), '');
  checked('cargo', ['generate-lockfile'], path);
  for (const [crate, version] of pins) {
    checked('cargo', ['update', '--package', crate, '--precise', version], path);
  }
  git(path, 'init', '--quiet');
  git(path, 'add', 'Cargo.toml', 'Cargo.lock', 'src/lib.rs');
  git(path, 'commit', '--quiet', '-m', 'example');
  return path;
}
const app = checkout('example-app', 'webpki-roots = "1"', [['webpki-roots', '1.0.9']]);
const legacy = checkout('example-legacy', 'rustls-webpki = "=0.101.7"', [['rustls-webpki', '0.101.7']]);
const locked = (path, crate) =>
  readFileSync(join(path, 'Cargo.lock'), 'utf8').match(new RegExp(`name = "${crate}"\\nversion = "(?<version>[^"]+)"`)).groups.version;

const env = { ...toolchain, HOME: home, STADO_CONFIG: join(root, 'tests', 'config', 'empty.json'), TMPDIR: join(home, 'tmp') };
const runs = [];
function advisoriesIn(environment, ...args) {
  const answer = run(stado, ['product', '--workspace', workspace, 'advisories', ...args], root, environment);
  runs.push({ args, exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr });
  writeFileSync(join(area, 'report.json'), JSON.stringify(runs));
  return answer;
}
const advisories = (...args) => advisoriesIn(env, ...args);
function refused(answer, sentence) {
  assert.ok(failed(answer), answer.stdout);
  assert.ok(answer.stderr.includes(sentence), `expected ${JSON.stringify(sentence)} in: ${answer.stderr}`);
}
const standing = (answer) => assert.ok(failed(answer), answer.stderr);
const at = (answer, checkoutPath, crate) =>
  answer.findings.find((finding) => finding.checkout === checkoutPath && finding.package === crate);

// Refusals before anything is read.
refused(advisories('list', 'example-missing'), 'names no product example-missing');
refused(advisories('list', '--database', 'not a url'), '--database "not a url" is not a URL; give an https origin');
refused(
  advisories('list', '--advisory', 'RUSTSEC-0000-0000'),
  'holds no crates.io advisory RUSTSEC-0000-0000; name an id or alias it publishes',
);
// RUSTSEC-2025-0007 is published and withdrawn: it is named as withdrawn,
// not as unknown.
refused(advisories('list', '--advisory', 'RUSTSEC-2025-0007'), 'withdrew RUSTSEC-2025-0007 at ');

// list: both committed locks are read and each affected version is named.
const listed = advisories('list', '--advisory', 'RUSTSEC-2026-0381', '--advisory', 'RUSTSEC-2026-0382', '--json');
standing(listed);
const before = JSON.parse(listed.stdout);
assert.deepEqual(before.checkouts.map((entry) => [entry.checkout, entry.read, entry.lockfiles, entry.uncommitted]), [
  [app, true, ['Cargo.lock'], []],
  [legacy, true, ['Cargo.lock'], []],
]);
const roots = at(before, app, 'webpki-roots');
assert.equal(roots.version, '1.0.9');
assert.equal(roots.advisory, 'RUSTSEC-2026-0381');
assert.equal(roots.compatible_fix, '1.0.10');
assert.equal(roots.working, null);
const pinned = at(before, legacy, 'rustls-webpki');
assert.equal(pinned.version, '0.101.7');
assert.equal(pinned.compatible_fix, null);
assert.deepEqual(pinned.dependents, ['example-legacy 0.1.0']);

// A run from a git hook or with an index of its own carries GIT_DIR and
// GIT_INDEX_FILE; each checkout is still read as itself.
const bound = advisoriesIn(
  { ...env, GIT_DIR: join(legacy, '.git'), GIT_INDEX_FILE: join(area, 'foreign-index') },
  'list', '--advisory', 'RUSTSEC-2026-0381', '--advisory', 'RUSTSEC-2026-0382', '--json',
);
standing(bound);
assert.deepEqual(JSON.parse(bound.stdout).checkouts, before.checkouts);
assert.deepEqual(JSON.parse(bound.stdout).findings, before.findings);

// apply on an uncommitted lockfile is refused and leaves it as it was; the
// lockfile is named as differing from its last commit.
appendFileSync(join(app, 'Cargo.lock'), '\n');
const dirty = advisories('apply', '--advisory', 'RUSTSEC-2026-0381', '--json');
standing(dirty);
const held = JSON.parse(dirty.stdout);
const appHeld = held.updates.find((update) => update.checkout === app);
assert.equal(appHeld.state, 'refused');
assert.ok(appHeld.cause.includes('has uncommitted changes'), appHeld.cause);
assert.deepEqual(held.checkouts.find((entry) => entry.checkout === app).uncommitted, ['Cargo.lock']);
assert.deepEqual(at(held, app, 'webpki-roots').working, ['1.0.9']);
assert.equal(locked(app, 'webpki-roots'), '1.0.9');
git(app, 'checkout', '--', 'Cargo.lock');

// apply locks the compatible fix on disk and names the unfixed one. It
// commits nothing, so the committed findings still stand, each naming what
// its working file locks now.
const head = git(app, 'rev-parse', 'HEAD');
const applied = advisories('apply', '--advisory', 'RUSTSEC-2026-0381', '--advisory', 'RUSTSEC-2026-0382', '--json');
standing(applied);
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
assert.deepEqual(after.findings.map((finding) => [finding.checkout, finding.package]), [
  [app, 'webpki-roots'],
  [legacy, 'rustls-webpki'],
]);
assert.deepEqual(at(after, app, 'webpki-roots').working, ['1.0.10']);
assert.equal(at(after, legacy, 'rustls-webpki').working, null);

// list keeps reading the committed lock until the fix is committed, and
// says the working file differs.
const uncommitted = advisories('list', '--advisory', 'RUSTSEC-2026-0381');
standing(uncommitted);
assert.ok(
  uncommitted.stdout.includes(`${join(app, 'Cargo.lock')} differs from its last commit; the findings read the committed one`),
  uncommitted.stdout,
);
assert.ok(
  uncommitted.stdout.includes('webpki-roots 1.0.9: RUSTSEC-2026-0381')
    && uncommitted.stdout.includes('its working file, not committed, locks 1.0.10'),
  uncommitted.stdout,
);

// An advisory is named by its id or any alias: RUSTSEC-2026-0381 lists
// RUSTSEC-2026-0382 among its aliases, so the legacy lock still stands once
// the fix is committed.
git(app, 'commit', '--quiet', '-am', 'lock the fix');
const committed = advisories('list', '--advisory', 'RUSTSEC-2026-0381', '--json');
standing(committed);
assert.deepEqual(JSON.parse(committed.stdout).findings.map((finding) => finding.checkout), [legacy]);

// The fixed rustls-webpki requires a newer aws-lc-rs, so Cargo moves it too,
// and apply names that move, as the rewritten lock holds it, beside the fix.
const verifier = checkout('example-verifier', 'rustls-webpki = { version = "0.103", features = ["aws-lc-rs"] }', [
  ['rustls-webpki', '0.103.13'],
  ['aws-lc-rs', '1.17.3'],
]);
const moving = advisories('apply', '--advisory', 'RUSTSEC-2026-0382', '--json');
standing(moving);
const fixed = JSON.parse(moving.stdout).updates.find((update) => update.checkout === verifier);
assert.equal(fixed.state, 'updated');
assert.equal(fixed.from, '0.103.13');
assert.equal(fixed.to, '0.103.16');
assert.equal(locked(verifier, 'rustls-webpki'), '0.103.16');
const crypto = fixed.moved.find((entry) => entry.package === 'aws-lc-rs');
assert.ok(crypto, JSON.stringify(fixed.moved));
assert.deepEqual(crypto.from, ['1.17.3']);
assert.deepEqual(crypto.to, [locked(verifier, 'aws-lc-rs')]);
assert.notDeepEqual(crypto.to, crypto.from);
assert.equal(fixed.moved.find((entry) => entry.package === 'rustls-webpki'), undefined, JSON.stringify(fixed.moved));
git(verifier, 'commit', '--quiet', '-am', 'lock the fix');

// Once the workspace holds only fixed and committed checkouts, list settles.
rmSync(legacy, { recursive: true });
const settled = advisories('list', '--advisory', 'RUSTSEC-2026-0381', '--json');
assert.ok(!failed(settled), settled.stderr);
assert.deepEqual(JSON.parse(settled.stdout).findings, []);

// The workspace and home go; the report of every run stays as the evidence.
rmSync(workspace, { recursive: true });
rmSync(home, { recursive: true });
console.log(join(area, 'report.json'));
