import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real qualification of `stado product cargo --all --json results` over the
// cargo area an installation leaves: beside the receipts, named by their
// invocation's UUID, the area holds the installation's Cargo cache under a
// platform directory whose subdirectories are named by digests and hold no
// receipt. One isolated home under this repository's `.build`, its
// workspace holding one checkout whose cargo area carries:
//
// - a finished receipt, answered with its state;
// - an invocation that wrote no result.json, answered as unfinished;
// - a receipt that is not JSON, named under `unread` with its cause;
// - the platform directory with digest-named subdirectories and no receipt,
//   named under `skipped` as no receipt and not under `unread`.
//
// The list answers with the readable receipts; no entry refuses it, with
// --json or without: the plain answer prints the unfinished receipt and
// names the unread one on stderr, and exits zero.
//
// Required: STADO_BIN, the Stado under test (a path, or a name on PATH).
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN is required: it names the Stado whose cargo results this test qualifies');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-cargo-results-'));
const home = join(output, 'home');
const workspace = join(home, 'workspace');
const checkout = join(workspace, 'results-fixture');
const report = { started_at: new Date().toISOString(), home, workspace, checkout, commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const environment = { ...process.env, HOME: home, WISENT_WORKSPACE: workspace };
function run(program, args) {
  const answer = spawnSync(program, args, { cwd: root, env: environment, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
const succeeded = answer => !answer.error && !answer.signal && !answer.status;
function success(answer) {
  assert.ok(succeeded(answer), answer.stderr || answer.error?.message || answer.signal);
  return answer.stdout.trim();
}
let failure;
try {
  report.source_revision = success(spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  const binary = stado.includes('/') ? resolve(stado) : success(run('which', [stado]));
  report.stado = { path: binary, sha256: digest(readFileSync(binary)), version: success(run(binary, ['--version'])) };

  // The checkout's cargo area, where Stado keeps a checkout's builds: its
  // path below the home, under ~/.stado/products/builds.
  mkdirSync(checkout, { recursive: true });
  const area = join(home, '.stado/products/builds', relative(home, checkout), 'cargo');
  const finished = join(area, randomUUID());
  mkdirSync(finished, { recursive: true });
  writeFileSync(join(finished, 'result.json'), JSON.stringify({ state: 'succeeded', operation: 'build' }));
  const running = join(area, randomUUID());
  mkdirSync(running, { recursive: true });
  const damaged = join(area, randomUUID());
  mkdirSync(damaged, { recursive: true });
  writeFileSync(join(damaged, 'result.json'), '{"state":');
  const platform = join(area, 'darwin-arm64');
  mkdirSync(join(platform, digest('lock'), digest('revision'), 'recipe'), { recursive: true });
  report.area = area;

  const listed = JSON.parse(success(run(binary, ['product', 'cargo', '--all', '--json', 'results'])));
  report.answer = listed;
  const row = path => listed.results.find(result => result.path === path);
  assert.equal(row(join(finished, 'result.json'))?.state, 'succeeded', 'the finished receipt was not answered');
  assert.equal(row(join(running, 'result.json'))?.state, 'unfinished', 'the invocation without a result was not listed unfinished');
  assert.equal(row(join(damaged, 'result.json')), undefined, 'a receipt that is not JSON was answered as a result');
  const named = (field, path) => {
    const entry = listed[field].find(candidate => candidate.path === path);
    assert.ok(entry, `${path} is not named under ${field}`);
    return entry.cause;
  };
  assert.match(named('unread', join(damaged, 'result.json')), /not JSON/);
  assert.match(named('skipped', platform), /not a receipt/);
  assert.equal(listed.unread.find(entry => entry.path === platform), undefined,
    'the platform cache was named as an unread receipt');

  const plain = run(binary, ['product', 'cargo', '--all', 'results']);
  report.plain = { status: plain.status, stdout: plain.stdout, stderr: plain.stderr };
  assert.equal(plain.status, 0, `the plain answer refused: ${plain.stderr}`);
  assert.ok(plain.stdout.includes(`unfinished ${join(running, 'result.json')}`), 'the plain answer left out the unfinished receipt');
  const damagedReceipt = join(damaged, 'result.json');
  const unreadLine = plain.stderr.split('\n').find(line => line.startsWith(`unread ${damagedReceipt}: `));
  assert.ok(unreadLine, `the plain answer did not name the unread receipt as text: ${plain.stderr}`);
  assert.match(unreadLine, /not JSON/, 'the plain unread line left out its cause');
  report.result = 'passed';
} catch (error) {
  report.error = error.stack || String(error);
  failure = error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'One isolated home and checkout cargo area with a finished, an unfinished and a damaged receipt and a platform cache directory; product cargo --all results only';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '  ')}\n`);
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
if (failure) throw failure;
