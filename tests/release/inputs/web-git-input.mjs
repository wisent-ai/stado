// `stado web quality` on a product whose lockfile pins private GitHub
// dependencies, through the real binary and a real `npm ci`. A dependency a
// declared Git bundle input answers installs from that bundle with no token;
// one no input answers, with no GITHUB_TOKEN, is refused naming it before
// npm runs, and nothing is installed. GitHub's Git origin is read from the
// declaration Stado itself reads, and the lockfile format from the npm that
// installs it. No network, credential or operator store is used.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly, exitedRefusing } from '../../native/process.mjs';

const binary = realpathSync(process.env.STADO_BIN ?? assert.fail('STADO_BIN names the stado binary under test'));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const github = JSON.parse(readFileSync(join(root, 'stado-rs/src/cli/web/builds/tooling/inputs/github.json'), 'utf8'));
const sshOrigin = `ssh://${github.sshUser}@${github.host}/`;
const parent = join(root, '.build', 'web-git-input');
mkdirSync(parent, { recursive: true });
const output = mkdtempSync(join(parent, 'run-'));
const fixture = join(output, 'fixture');
const home = join(fixture, 'home');
for (const directory of [home, join(fixture, 'temporary')]) mkdirSync(directory, { recursive: true });
const environment = {
  PATH: process.env.PATH, HOME: home, TMPDIR: join(fixture, 'temporary'),
  GIT_CONFIG_NOSYSTEM: 'true', GIT_ALLOW_PROTOCOL: 'file',
  GIT_AUTHOR_NAME: 'journey', GIT_AUTHOR_EMAIL: 'journey@example.invalid',
  GIT_COMMITTER_NAME: 'journey', GIT_COMMITTER_EMAIL: 'journey@example.invalid',
  npm_config_cache: join(fixture, 'npm-cache'), npm_config_logs_dir: join(output, 'npm-logs'),
  npm_config_audit: 'false', npm_config_fund: 'false', npm_config_update_notifier: 'false',
};
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const report = { started_at: new Date().toISOString(), binary, binary_sha256: digest(readFileSync(binary)), commands: [], cases: [], verdict: 'failed' };

function command(program, args, cwd, extra = {}) {
  const directory = join(output, 'commands', randomUUID());
  mkdirSync(directory, { recursive: true });
  const stdout = join(directory, 'stdout');
  const stderr = join(directory, 'stderr');
  const out = openSync(stdout, 'w');
  const err = openSync(stderr, 'w');
  let result;
  try { result = spawnSync(program, args, { cwd, env: { ...environment, ...extra }, stdio: ['ignore', out, err] }); }
  finally { closeSync(out); closeSync(err); }
  report.commands.push({ program, args, cwd, extra: Object.keys(extra), status: result.status, signal: result.signal, error: result.error?.message, stdout, stderr });
  return { ...result, stdout: readFileSync(stdout, 'utf8'), stderr: readFileSync(stderr, 'utf8') };
}
function success(program, args, cwd, extra) {
  const result = command(program, args, cwd, extra);
  assert.ok(exitedCleanly(result), `${program} ${args.join(' ')}: ${result.stderr}`);
  return result.stdout;
}

// The lockfile format this npm writes and `npm ci` reads, measured by having
// it lock a package with no dependencies.
function lockfileFormat() {
  const probe = join(fixture, 'lockfile-format');
  mkdirSync(probe, { recursive: true });
  writeFileSync(join(probe, 'package.json'), JSON.stringify({ name: 'lockfile-format', version: '1.0.0', private: true }));
  success('npm', ['install', '--package-lock-only', '--offline'], probe);
  return JSON.parse(readFileSync(join(probe, 'package-lock.json'), 'utf8')).lockfileVersion;
}

// A dependency repository with one committed file, its bundle and its revision.
function dependency(name) {
  const source = join(fixture, `${name}-source`);
  mkdirSync(source, { recursive: true });
  success('git', ['init', '--quiet', '--initial-branch=main'], source);
  writeFileSync(join(source, 'package.json'), JSON.stringify({ name, version: '1.0.0' }));
  writeFileSync(join(source, 'index.js'), `module.exports = ${JSON.stringify(name)};\n`);
  success('git', ['add', '.'], source);
  success('git', ['commit', '--quiet', '--message', `${name} fixture`], source);
  const revision = success('git', ['rev-parse', 'HEAD'], source).trim();
  const bundle = join(fixture, `${name}.bundle`);
  success('git', ['bundle', 'create', bundle, '--all'], source);
  return { name, revision, bundle, repository: `example-owner/${name}.git`, url: `${sshOrigin}example-owner/${name}.git` };
}

// A web product whose lockfile pins `dependencies` over GitHub's SSH origin.
function product(label, dependencies, lockfileVersion) {
  const checkout = join(fixture, label);
  mkdirSync(checkout, { recursive: true });
  const pinned = Object.fromEntries(dependencies.map(dep => [dep.name, `git+${dep.url}#${dep.revision}`]));
  writeFileSync(join(checkout, 'package.json'), JSON.stringify({ name: 'example-web-input', version: '1.0.0', private: true, scripts: { build: 'true' }, dependencies: pinned }));
  const packages = { '': { name: 'example-web-input', version: '1.0.0', dependencies: pinned } };
  for (const dep of dependencies) packages[`node_modules/${dep.name}`] = { version: '1.0.0', resolved: `git+${dep.url}#${dep.revision}` };
  writeFileSync(join(checkout, 'package-lock.json'), JSON.stringify({ name: 'example-web-input', version: '1.0.0', lockfileVersion, requires: true, packages }));
  return checkout;
}

// `stado web quality` as the release worker runs it on `checkout`, each of
// `answered` declared as a Git bundle input.
function quality(checkout, answered) {
  const worker = { WISENT_SOURCE_DIR: checkout, WISENT_OUTPUT_DIR: join(checkout, '.release'), WISENT_VERSION: '1.0.0', WISENT_PLATFORM: 'web' };
  const args = ['web', 'quality'];
  for (const dep of answered) {
    worker[`WISENT_INPUT_${dep.name.toUpperCase().replace(/[^A-Z0-9]/g, '_')}_DIR`] = dep.bundle;
    args.push('--git-input', `${dep.name}=${dep.repository}`);
  }
  return command(binary, args, checkout, worker);
}

try {
  report.source_revision = success('git', ['rev-parse', 'HEAD'], root).trim();
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.npm_version = success('npm', ['--version'], root).trim();
  const format = lockfileFormat();
  const first = dependency('example-first');
  const second = dependency('example-second');

  const answered = product('answered', [first], format);
  const passed = quality(answered, [first]);
  assert.ok(exitedCleanly(passed), `quality refused a dependency its input answers: ${passed.stderr}`);
  assert.ok(passed.stdout.includes('the declared release inputs answer every one'), passed.stdout);
  const installed = join(answered, 'node_modules', first.name, 'index.js');
  assert.equal(readFileSync(installed, 'utf8'), `module.exports = ${JSON.stringify(first.name)};\n`);
  report.cases.push({ name: 'input-answers-every-pin', verdict: 'passed', installed });

  const unanswered = product('unanswered', [first, second], format);
  const refused = quality(unanswered, [first]);
  assert.ok(exitedRefusing(refused), `quality accepted a pin no input answers: ${refused.stdout}`);
  assert.ok(refused.stderr.includes('no release input answers'), refused.stderr);
  assert.ok(refused.stderr.includes(second.url), `the refusal does not name ${second.url}: ${refused.stderr}`);
  assert.ok(!refused.stderr.includes(first.url), `the refusal names the answered ${first.url}: ${refused.stderr}`);
  assert.ok(!existsSync(join(unanswered, 'node_modules')), 'npm ran after the refusal');
  report.cases.push({ name: 'unanswered-pin-refused-by-name', verdict: 'passed' });
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error);
} finally {
  report.finished_at = new Date().toISOString();
  rmSync(fixture, { recursive: true, force: true });
  writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
if (report.verdict !== 'passed') throw new Error(report.error);
