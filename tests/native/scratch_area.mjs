import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  constants, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync,
} from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of where `stado product swift` builds: the Stado under
// test builds and indexes a real Swift package in a canonical checkout of an
// isolated workspace and home, with the real Swift toolchain. SwiftPM's build
// tree must land in the checkout's build area under the home,
// `.stado/products/builds/<the checkout's path below the home>/swift/build`,
// tagged with CACHEDIR.TAG, and nothing of it in the package; the index must
// read the relocated tree and publish the package's own source as not
// generated. Everything lives below this checkout's ignored .build and is
// removed at the end. Required: STADO_BIN, the Stado executable under test.
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: the Stado executable under test');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'native-scratch-area-'));
const home = join(output, 'home');
const workspace = join(home, 'workspace');
const report = { started_at: new Date().toISOString(), commands: [], observations: [], result: 'failed' };
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const signature = 'Signature: 8a477f597d28d172789f06886806bc55';
function run(program, args, cwd = root, environment = process.env) {
  const answer = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const SUCCESS = run('/usr/bin/true', []).status;
function success(answer) {
  assert.equal(answer.status, SUCCESS, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
const isolated = { ...process.env, HOME: home, WISENT_WORKSPACE: workspace };
function swift(operation, packagePath) {
  return JSON.parse(success(run(stado,
    ['product', '--workspace', workspace, 'swift', '--package-path', packagePath, '--json', operation],
    packagePath, isolated)));
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: stado, sha256: digest(readFileSync(stado)), version: success(run(stado, ['--version'])) };
  const checkout = join(workspace, 'example-swift');
  mkdirSync(join(checkout, 'Sources/ExampleKit'), { recursive: true });
  writeFileSync(join(checkout, 'Package.swift'),
    '// swift-tools-version:5.9\nimport PackageDescription\n\nlet package = Package(\n    name: "ExampleKit",\n    targets: [.target(name: "ExampleKit")]\n)\n');
  writeFileSync(join(checkout, 'Sources/ExampleKit/Greeting.swift'),
    'public func greeting() -> String { "hello" }\n');
  success(run('git', ['init', '--quiet', '--initial-branch=main', checkout]));
  success(run('git', ['-C', checkout, 'config', 'remote.origin.url', 'https://github.com/example-org/example-swift.git']));
  success(run('git', ['-C', checkout, 'add', '.']));
  success(run('git', ['-C', checkout, '-c', 'user.name=Native scratch test', '-c', 'user.email=native@example.invalid',
    'commit', '--quiet', '-m', 'Example package']));
  const area = join(realpathSync(home), '.stado/products/builds',
    relative(realpathSync(home), realpathSync(checkout)), 'swift');

  const built = swift('build', checkout);
  report.observations.push({ operation: 'build', value: built });
  assert.equal(built.state, 'succeeded', built.error);
  assert.equal(built.scratch_path, join(area, 'build'));
  assert.ok(readFileSync(join(area, 'build/CACHEDIR.TAG'), 'utf8').startsWith(`${signature}\n`),
    'the build area carries no CACHEDIR.TAG');
  assert.ok(existsSync(join(area, 'build/debug')), 'SwiftPM wrote no build output into the build area');
  assert.ok(!existsSync(join(checkout, '.build/wisent-native/build')), 'the build tree was written into the package');

  const indexed = swift('index', checkout);
  report.observations.push({ operation: 'index', value: indexed });
  assert.equal(indexed.state, 'succeeded', indexed.error);
  const settings = JSON.parse(readFileSync(join(checkout, '.build/wisent-native/index/build-settings.json'), 'utf8'));
  const sources = settings.targets.flatMap((target) => target.sources);
  const own = sources.find((source) => source.uri.endsWith('/Sources/ExampleKit/Greeting.swift'));
  assert.ok(own, `the index lists no package source: ${JSON.stringify(sources)}`);
  assert.equal(own.generated, false);
  assert.ok(!existsSync(join(checkout, '.build/wisent-native/build')), 'the index wrote a build tree into the package');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  throw error;
} finally {
  rmSync(home, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  report.scope = 'A real Swift package built and indexed by the Stado under test with the real toolchain in an isolated home and workspace';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
