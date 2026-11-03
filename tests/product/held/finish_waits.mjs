import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { pinnedGit } from '../../native/home.mjs';
import { fileURLToPath } from 'node:url';

// Real journey of the handoff an installation of Stado itself makes: its
// build passes, it lets go of the surface's lock and its freshly built Stado
// runs `product finish`, which takes the lock again. An installation that
// waited for the lock (`--wait`, as `product sync` always does) must hand off
// a finish that waits too; before the repair the finish superseded whatever
// held the lock by then and told it to stop.
//
// The Stado under test runs with HOME set to an isolated directory under
// .build, so its receipts and locks are never the person's; the workspace is
// the real one, read for Stado's checkout.
//
// 1. A Stado install with --wait (`first`) at STADO_FINISH_TEST_COMMIT starts.
// 2. Once it says its build passed and hands off, a second install with
//    --wait (`second`) of the commit before it is started; it takes the lock
//    while the finish has not yet, or waits behind the finish.
// 3. Neither ends on SIGTERM, and no stderr says `superseding the installation`.
// Required: STADO_BIN, STADO_FINISH_TEST_SOURCE (Stado's checkout),
// STADO_FINISH_TEST_COMMIT (a commit on its origin/main whose parent builds).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
const source = process.env.STADO_FINISH_TEST_SOURCE;
const commit = process.env.STADO_FINISH_TEST_COMMIT;
assert.ok(stado && source && commit,
  'STADO_BIN, STADO_FINISH_TEST_SOURCE and STADO_FINISH_TEST_COMMIT are required');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-finish-waits-'));
const workspace = dirname(realpathSync(source));
const home = join(output, 'home');
mkdirSync(join(home, '.stado'), { recursive: true });
const env = { ...process.env, ...pinnedGit, HOME: home };
const report = { started_at: new Date().toISOString(), source, commit, workspace, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status, signal: answer.signal,
    stdout: answer.stdout, stderr: answer.stderr });
  assert.ok(!answer.status && !answer.signal, answer.stderr);
  return answer.stdout.trim();
}
const children = [];
function started(args) {
  const child = spawn(resolve(stado), args, { cwd: root, env });
  const kept = { arguments: args, pid: child.pid, stderr: '' };
  child.stderr.on('data', chunk => { kept.stderr += chunk; });
  child.ended = new Promise(done => child.on('exit', (code, signal) => {
    Object.assign(kept, { exit_status: code, signal });
    done(kept);
  }));
  report.commands.push(kept);
  children.push(child);
  return { child, kept };
}
async function said(one, text) {
  while (!one.kept.stderr.includes(text)) {
    if (one.child.exitCode !== null || one.child.signalCode !== null) {
      throw new Error(`pid ${one.child.pid} ended before saying ${JSON.stringify(text)}: ${one.kept.stderr.slice(-2000)}`);
    }
    await new Promise(next => one.child.stderr.once('data', next));
  }
}
let failure;
try {
  report.source_revision = run('git', ['rev-parse', 'HEAD']);
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  report.stado = { sha256: digest(readFileSync(resolve(stado))), version: run(resolve(stado), ['--version']) };
  const earlier = run('git', ['-C', source, 'rev-parse', `${commit}^`]);
  const install = revision => ['product', '--workspace', workspace, 'install', 'stado', '--surface', 'cli',
    '--source-commit', revision, '--wait'];

  const first = started(install(commit));
  await said(first, 'the build passed and is retained');
  const second = started(install(earlier));
  const [one, two] = await Promise.all([first.child.ended, second.child.ended]);
  for (const ended of [one, two]) {
    assert.notEqual(ended.signal, 'SIGTERM', `pid ${ended.pid} was told to stop: ${ended.stderr.slice(-2000)}`);
    assert.notEqual(ended.exit_status, 143, `pid ${ended.pid} ended 143: ${ended.stderr.slice(-2000)}`);
    assert.doesNotMatch(ended.stderr, /superseding the installation/, `pid ${ended.pid} superseded a waiting install`);
  }
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  for (const child of children) {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  }
  report.finished_at = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
