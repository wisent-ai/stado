import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of where a kompilator build puts a binary: the kompilator
// under test builds a package of one binary and no dependencies under .build,
// and a caller learns the binary's path only from kompilator's own answer.
// `where` before any build refuses naming the missing path and the build
// command that writes it; a flag that changes what is compiled and a binary
// the package does not declare are refused; `build` prints the path of the
// binary it linked, which runs; `where` then answers that same path; and
// `--out` moves both answers. Required: KOMPILATOR_BIN, the kompilator under
// test (`rustc --edition 2021 -O kompilator/src/main.rs -o <path>`), and
// rustc on PATH.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const kompilator = process.env.KOMPILATOR_BIN;
assert.ok(kompilator, 'KOMPILATOR_BIN names the kompilator under test');
mkdirSync(join(root, '.build'), { recursive: true });
const area = mkdtempSync(join(realpathSync(join(root, '.build')), 'kompilator-where-'));
const pkg = join(area, 'example');
mkdirSync(join(pkg, 'src'), { recursive: true });
writeFileSync(join(pkg, 'Cargo.toml'), '[package]\nname = "example"\nversion = "0.1.0"\nedition = "2021"\n');
writeFileSync(join(pkg, 'Cargo.lock'), 'version = 4\n\n[[package]]\nname = "example"\nversion = "0.1.0"\n');
writeFileSync(join(pkg, 'src', 'main.rs'), 'fn main() { println!("example ran"); }\n');
const manifest = join(pkg, 'Cargo.toml');

const runs = [];
function kompile(...args) {
  const answer = spawnSync(kompilator, args, { cwd: area, env: process.env, encoding: 'utf8' });
  assert.ok(!answer.error && !answer.signal, `kompilator ${args.join(' ')}: ${answer.error?.message}`);
  runs.push({ args, exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr });
  writeFileSync(join(area, 'report.json'), JSON.stringify(runs, null, 2));
  return answer;
}
function refused(answer, status, sentence) {
  assert.equal(answer.status, status, answer.stderr);
  assert.equal(answer.stdout, '', 'a refusal prints no path');
  assert.ok(answer.stderr.includes(sentence), `expected ${JSON.stringify(sentence)} in: ${answer.stderr}`);
}
const released = join(pkg, '.build', 'kompilator', 'release', 'example');

// Before any build: the path is named as missing, with the command that writes it.
refused(
  kompile('where', '--manifest-path', manifest, '--release', '--bin', 'example'),
  1,
  `example has not been built: ${released} does not exist; \`kompilator build --manifest-path ${manifest} --release --bin example\` builds it there`,
);
refused(
  kompile('where', '--manifest-path', manifest, '--jobs', '2'),
  2,
  'kompilator where does not take --jobs: it reads only --manifest-path, --release, --bin and --out',
);
refused(
  kompile('where', '--manifest-path', manifest, '--bin', 'missing'),
  1,
  '--bin missing: example 0.1.0 declares no such binary; it declares example',
);
refused(kompile('where'), 2, 'kompilator where needs --manifest-path');

// build answers the path of the binary it linked, and that binary runs.
const built = kompile('build', '--manifest-path', manifest, '--release', '--bin', 'example');
assert.equal(built.status, 0, built.stderr);
assert.equal(built.stdout, `${released}\n`);
const ran = spawnSync(released, [], { encoding: 'utf8' });
assert.equal(ran.status, 0, ran.stderr);
assert.equal(ran.stdout, 'example ran\n');

// where now answers the same path without building.
const found = kompile('where', '--manifest-path', manifest, '--release', '--bin', 'example');
assert.equal(found.status, 0, found.stderr);
assert.equal(found.stdout, `${released}\n`);

// --out moves the binary, and both answers follow it.
const elsewhere = join(area, 'elsewhere');
const moved = kompile('build', '--manifest-path', manifest, '--out', elsewhere);
assert.equal(moved.status, 0, moved.stderr);
assert.equal(moved.stdout, `${join(elsewhere, 'example')}\n`);
assert.ok(existsSync(join(elsewhere, 'example')));
const movedFound = kompile('where', '--manifest-path', manifest, '--out', elsewhere);
assert.equal(movedFound.stdout, `${join(elsewhere, 'example')}\n`);

// The package and its builds go; the report of every run stays as evidence.
rmSync(pkg, { recursive: true });
rmSync(elsewhere, { recursive: true });
console.log(join(area, 'report.json'));
