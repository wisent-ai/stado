import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  chmodSync, constants, existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync,
  realpathSync, rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado product build-trees list|remove`: the Stado
// under test reads a workspace of real git checkouts, lists every rebuildable
// tree with its bytes, removes those no build holds and keeps everything
// else. The workspace is a dedicated directory below this checkout's ignored
// .build. One checkout's origin is a catalog desktop repository, so its
// SwiftPM `.build` is Stado's to remove; the other's is not, and stays. A
// Cargo build lock is held by a real `lockf` process for as long as the test
// needs the tree in use. Required: STADO_BIN, the Stado executable under test.
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: the Stado executable under test');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-build-trees-'));
const workspace = join(output, 'workspace');
const outside = join(output, 'outside-cache');
const report = { started_at: new Date().toISOString(), commands: [], observations: [], result: 'failed' };
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const signature = 'Signature: 8a477f597d28d172789f06886806bc55';
const escaped = (text) => text.replace(/[.()]/g, '\\$&');
function run(program, args, cwd = root) {
  const answer = spawnSync(program, args, { cwd, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
// The status a program that succeeded exits with, as this machine reports it.
const SUCCESS = run('/usr/bin/true', []).status;
function success(answer) {
  assert.equal(answer.status, SUCCESS, answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
function trees(operation) {
  return JSON.parse(success(run(stado, ['product', '--workspace', workspace, 'build-trees', operation, '--json'])));
}
function write(path, text) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
}
function cache(path, payload) {
  write(join(path, 'CACHEDIR.TAG'), `${signature}\n# A regenerable test cache.\n`);
  write(join(path, 'payload'), payload);
}
// The length of every file below `path`, links counted as themselves.
function files(path) {
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    const child = join(path, entry.name);
    return entry.isDirectory() ? files(child) : [lstatSync(child).size];
  });
}
// What the command reports for a tree: the sum of those lengths.
const lengths = (path) => files(path).reduce((total, size) => total + size);
const created = [];
function checkout(name, origin) {
  const path = join(workspace, name);
  mkdirSync(path, { recursive: true });
  success(run('git', ['init', '--quiet', '--initial-branch=main', path]));
  success(run('git', ['-C', path, 'config', 'remote.origin.url', origin]));
  created.push(path);
  return path;
}
const row = (answer, path) => answer.trees.find((tree) => tree.path === path);
const inState = (answer, state) => answer.trees.filter((tree) => tree.state === state).map((tree) => tree.path);
let holder;
let locked;
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { path: stado, sha256: digest(readFileSync(stado)), version: success(run(stado, ['--version'])) };

  // A desktop product's checkout: its .build, a Cargo target, a former
  // Stado install run, and what is not rebuildable beside them.
  const desktop = checkout('example-desktop', 'https://github.com/wisent-ai/tama-desktop.git');
  write(join(desktop, '.build/release/Example'), 'swift build output\n');
  write(join(desktop, '.build/wisent-source/index'), 'a private index\n');
  cache(join(desktop, 'rust/target'), 'cargo build output\n');
  write(join(desktop, '.wisent-output/install/run-a/source/main.rs'), 'an exported source\n');
  write(join(desktop, '.wisent-output/install-failures/key.json'), '{"refused": true}\n');
  write(join(desktop, '.wisent-output/quality/report.txt'), 'evidence\n');
  write(join(desktop, 'Sources/main.swift'), 'print("source")\n');
  write(join(desktop, 'notes/CACHEDIR.TAG'), 'not a signature\n');
  write(join(desktop, 'notes/plan.md'), 'the user\'s notes\n');
  cache(outside, 'outside the workspace\n');
  symlinkSync(outside, join(desktop, 'linked-cache'));
  // Another checkout: its .build is no desktop product's, and a build holds
  // its Cargo target.
  const tool = checkout('example-tool', 'https://github.com/example-org/example-tool.git');
  write(join(tool, '.build/debug/tool'), 'not a desktop product\n');
  cache(join(tool, 'target'), 'held build output\n');
  const lock = join(tool, 'target/debug/.cargo-lock');
  write(lock, '');
  holder = spawn('/usr/bin/lockf', ['-k', lock, '/bin/cat'], { stdio: ['pipe', 'ignore', 'pipe'] });
  locked = new Promise((resolveExit) => holder.on('exit', resolveExit));
  // lockf starts cat only once it holds the lock; cat's process is the proof.
  for (let held = false; !held;) {
    held = run('/usr/bin/pgrep', ['-P', String(holder.pid), 'cat']).status === SUCCESS;
  }

  const expected = {
    [join(desktop, '.build')]: 'desktop_build_tree',
    [join(desktop, 'rust/target')]: 'cachedir_tag',
    [join(desktop, '.wisent-output/install')]: 'stado_run_area',
    [join(tool, 'target')]: 'cachedir_tag',
  };
  const sizes = Object.fromEntries(Object.keys(expected).map((path) => [path, lengths(path)]));
  report.observations.push({ operation: 'fixture sizes', value: sizes });

  const listed = trees('list');
  report.observations.push({ operation: 'list', value: listed });
  assert.equal(listed.checkouts, created.length);
  assert.deepEqual(listed.trees.map((tree) => tree.path).sort(), Object.keys(expected).sort(),
    'exactly the declared and tagged trees are listed, a linked cache and a false tag are not');
  for (const [path, declared] of Object.entries(expected)) {
    assert.equal(row(listed, path).declared_by, declared, path);
    assert.equal(row(listed, path).bytes, sizes[path], `${path} bytes`);
  }
  assert.deepEqual(inState(listed, 'in_use'), [join(tool, 'target')]);
  assert.equal(row(listed, join(tool, 'target')).lock, lock);
  for (const path of Object.keys(expected)) assert.ok(existsSync(path), `list removed ${path}`);

  const removed = trees('remove');
  report.observations.push({ operation: 'remove while a build holds a target', value: removed });
  const reclaimable = Object.keys(expected).filter((path) => path !== join(tool, 'target'));
  assert.deepEqual(inState(removed, 'removed').sort(), reclaimable.sort());
  assert.equal(removed.removed_bytes, reclaimable.map((path) => sizes[path]).reduce((total, size) => total + size));
  assert.equal(removed.in_use_bytes, sizes[join(tool, 'target')]);
  for (const path of reclaimable) assert.ok(!existsSync(path), `${path} survived remove`);
  assert.deepEqual(inState(removed, 'in_use'), [join(tool, 'target')]);
  assert.equal(readFileSync(join(tool, 'target/payload'), 'utf8'), 'held build output\n');
  for (const kept of ['Sources/main.swift', 'notes/plan.md', 'notes/CACHEDIR.TAG',
    '.wisent-output/install-failures/key.json', '.wisent-output/quality/report.txt']) {
    assert.ok(existsSync(join(desktop, kept)), `${kept} was removed`);
  }
  assert.ok(existsSync(join(tool, '.build/debug/tool')), 'a .build of no desktop product was removed');
  assert.equal(readFileSync(join(outside, 'payload'), 'utf8'), 'outside the workspace\n');
  assert.ok(lstatSync(join(desktop, 'linked-cache')).isSymbolicLink(), 'the link itself was removed');

  holder.stdin.end();
  await locked;
  holder = undefined;
  const released = trees('remove');
  report.observations.push({ operation: 'remove once the build released its lock', value: released });
  assert.deepEqual(inState(released, 'removed'), [join(tool, 'target')]);
  assert.ok(!existsSync(join(tool, 'target')), 'a released target survived remove');
  assert.deepEqual(trees('list').trees, [], 'a second list finds nothing left');

  // A tree whose contents the account may not unlink stays, and the command
  // names it and fails.
  const stuckTree = join(tool, 'stuck/target');
  cache(stuckTree, 'build output in a directory nobody may write\n');
  const stuck = join(stuckTree, 'locked');
  write(join(stuck, 'artifact'), 'kept by its directory\'s mode\n');
  chmodSync(stuck, constants.S_IRUSR | constants.S_IXUSR);
  const refused = run(stado, ['product', '--workspace', workspace, 'build-trees', 'remove', '--json']);
  chmodSync(stuck, constants.S_IRWXU);
  report.observations.push({ operation: 'remove of a tree with an unwritable directory', value: refused.stdout });
  assert.notEqual(refused.status, SUCCESS, 'an unremovable tree must fail the command');
  assert.match(refused.stderr, new RegExp(`build-tree removal incomplete: \\d+ tree\\(s\\) under ${escaped(workspace)} could not be removed`));
  const failed = JSON.parse(refused.stdout);
  assert.deepEqual(inState(failed, 'failed'), [stuckTree]);
  assert.ok(failed.errors.some((error) => error.startsWith(`removing ${stuckTree}: `)), failed.errors.join('\n'));
  assert.ok(existsSync(join(stuck, 'artifact')), 'the refused file is gone');

  const missing = join(output, 'no-workspace');
  const absent = run(stado, ['product', '--workspace', missing, 'build-trees', 'list']);
  assert.notEqual(absent.status, SUCCESS);
  assert.match(absent.stderr, new RegExp(`reading the workspace ${escaped(missing)}`));
  const unknown = run(stado, ['product', '--workspace', workspace, 'build-trees', 'prune']);
  assert.match(unknown.stderr, /invalid value 'prune'/);
  const bare = run(stado, ['product', '--workspace', workspace, 'build-trees']);
  assert.match(bare.stderr, /Usage: stado product build-trees/);
  assert.notEqual(unknown.status, SUCCESS);
  assert.equal(bare.status, unknown.status, 'a missing and an unknown operation are the same usage error');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  throw error;
} finally {
  if (holder) {
    holder.stdin.end();
    await locked;
  }
  rmSync(workspace, { recursive: true, force: true });
  rmSync(outside, { recursive: true, force: true });
  report.finished_at = new Date().toISOString();
  report.scope = 'Real workspace checkouts read and cleared by the Stado under test; a real held Cargo lock; the refusals';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
