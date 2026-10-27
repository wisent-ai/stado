import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { access, appendFile, chmod, constants, copyFile, mkdir, readFile, readdir, realpath, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly, exitedRefusing } from '../../native/process.mjs';
import { snapshotSubject, verifyRevision } from '../../native/subject.mjs';

const source = fileURLToPath(new URL('../../../', import.meta.url));
// A quote is valid in a Git ref and requires JSON string decoding; the
// ordinary success path also proves that boundary without a duplicate run.
const branch = 'values"retained';
const reference = `refs/heads/${branch}`;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const required = name => {
  assert.ok(process.env[name], `qualification requires ${name}`);
  return process.env[name];
};

test('history reviews before the ref lock, refuses broken integration, and remains usable with ordinary Git', async t => {
  const area = join(source, '.build/release-history-preflight', randomUUID());
  const home = join(area, 'home');
  const repo = join(home, 'Documents/CodingProjects/Wisent/ledger');
  const upstream = join(area, 'upstream.git');
  const hooks = join(home, 'hooks');
  const trace = join(area, 'git-trace.jsonl');
  await mkdir(repo, { recursive: true });
  await mkdir(hooks);
  await mkdir(join(area, 'subject'));
  const report = { started_at: new Date().toISOString(), outcome: 'blocked', command_log: 'commands.jsonl', trace };
  const environment = { ...process.env };
  async function run(program, args, cwd = repo, checked = true) {
    const startedAt = new Date().toISOString();
    const stdout = [], stderr = [];
    const child = spawn(program, args, { cwd, env: environment, signal: t.signal, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout.on('data', data => stdout.push(data));
    child.stderr.on('data', data => stderr.push(data));
    const result = await new Promise(resolve => {
      child.once('error', error => resolve({ error: { name: error.name, message: error.message, code: error.code } }));
      child.once('close', (status, signal) => resolve({ status, signal }));
    });
    result.stdout = Buffer.concat(stdout).toString();
    result.stderr = Buffer.concat(stderr).toString();
    await appendFile(join(area, report.command_log), `${JSON.stringify({ program, args, cwd, started_at: startedAt, ...result })}\n`);
    if (checked) assert.ok(exitedCleanly(result), JSON.stringify({ program, args, ...result }));
    return result;
  }
  const git = async (...args) => (await run('git', args)).stdout.trim();
  try {
    // Git supplies its own local-environment identities; fixture isolation
    // does not guess which inherited values could redirect another checkout.
    const local = await git('rev-parse', '--local-env-vars');
    for (const name of local.split('\n').filter(Boolean)) delete environment[name];
    Object.assign(environment, {
      HOME: home,
      GIT_CONFIG_GLOBAL: join(home, '.gitconfig'),
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_TRACE2_EVENT: trace,
      OKO_DEFECT_REGISTER: join(home, 'defects.json'),
    });
    report.environment = { HOME: home, GIT_CONFIG_GLOBAL: environment.GIT_CONFIG_GLOBAL, GIT_TRACE2_EVENT: trace, OKO_DEFECT_REGISTER: environment.OKO_DEFECT_REGISTER };
    const expected = required('STADO_TEST_SOURCE_REVISION');
    report.expected_stado_revision = expected;
    assert.equal(await run('git', ['rev-parse', 'HEAD'], source).then(result => result.stdout.trim()), expected);
    await run('git', ['diff', '--quiet', 'HEAD'], source);
    const subject = snapshotSubject(required('STADO_BIN'), join(area, 'subject'));
    report.stado = subject;
    verifyRevision((await run(subject.path, ['--version'])).stdout, expected);
    const runtime = await realpath(required('TAMA_TEST_RUNTIME_ROOT'));
    const inventoryBytes = await readFile(join(runtime, 'native-hook-binaries.json'));
    const inventory = JSON.parse(inventoryBytes);
    const expectedTama = required('TAMA_TEST_SOURCE_REVISION');
    assert.equal(inventory.sourceIdentity?.dirty, false, 'Tama runtime must identify clean candidate source');
    assert.equal(inventory.sourceIdentity.revision, expectedTama, 'Tama runtime must match the declared candidate');
    const entry = inventory.hooks.find(item => item.name === 'tama' && item.package === 'tama-cli');
    assert.ok(entry, 'sealed runtime declares no native Tama CLI');
    const tama = await realpath(join(runtime, 'bin', entry.name));
    await access(tama, constants.X_OK);
    const tamaDigest = digest(await readFile(tama));
    report.tama = { runtime, binary: tama, sha256: tamaDigest, source_identity: inventory.sourceIdentity };
    await writeFile(join(area, 'native-hook-binaries.json'), inventoryBytes);
    await copyFile(join(runtime, 'release.json'), join(area, 'tama-release.json'));
    report.dependencies_ready = true;

    await run('git', ['init', '--bare', `--initial-branch=${branch}`, upstream], area);
    await git('init', `--initial-branch=${branch}`);
    await git('config', 'user.name', 'History preflight journey');
    await git('config', 'user.email', 'history@example.invalid');
    await git('remote', 'add', 'origin', upstream);
    await writeFile(join(repo, 'value.txt'), 'base\n');
    await git('add', 'value.txt');
    const base = await git('commit-tree', await git('write-tree'), '-m', 'Establish a value');
    await git('update-ref', reference, base);
    await run('git', ['fetch', repo, `${branch}:${branch}`], upstream);
    await git('fetch', 'origin');
    await git('branch', `--set-upstream-to=origin/${branch}`, branch);
    await writeFile(join(repo, 'value.txt'), 'base\nretained\n');
    await git('add', 'value.txt');
    const feature = await git('commit-tree', await git('write-tree'), '-p', base, '-m', 'Retain the next value');
    await git('update-ref', reference, feature, base);
    const tree = await git('rev-parse', 'HEAD^{tree}');
    const message = join(area, 'message.txt');
    const plan = join(area, 'plan.json');
    await writeFile(message, 'Describe the retained value\n');
    await writeFile(plan, JSON.stringify({ commits: [{ action: 'reword', commit: feature, message }] }));
    await run(tama, ['install', '--root', runtime, '--home', home, '--git-hooks-path', hooks, '--set-git-config', '--json']);
    const hook = join(hooks, 'reference-transaction');
    const original = await readFile(hook);
    const originalMode = (await stat(hook)).mode;
    const historyArgs = ['release', 'history', '--root', repo, '--plan', plan, '--json'];
    const rewritten = JSON.parse((await run(subject.path, historyArgs)).stdout);
    let current = await git('rev-parse', 'HEAD');
    assert.notEqual(current, feature);
    assert.equal(rewritten.from, feature);
    assert.equal(rewritten.to, current);
    assert.equal(await git('rev-parse', 'HEAD^{tree}'), tree);
    assert.equal(await git('log', '-1', '--format=%B'), 'Describe the retained value');
    assert.equal(await git('status', '--porcelain'), '');
    assert.equal((await run('git', ['rev-parse', branch], upstream)).stdout.trim(), base);
    const common = await git('rev-parse', '--path-format=absolute', '--git-common-dir');
    assert.deepEqual(await readdir(join(common, 'tama-ref-move-admissions')), [], 'a completed move must spend its admission');
    const stadoHead = current;
    await writeFile(message, 'Keep the retained value understandable\n');
    await run(tama, ['format', 'fold', '--repository', repo, '--reword', current, '--message', message, '--json']);
    current = await git('rev-parse', 'HEAD');
    assert.notEqual(current, stadoHead);
    assert.equal(await git('rev-parse', 'HEAD^{tree}'), tree);
    assert.equal(await git('log', '-1', '--format=%B'), 'Keep the retained value understandable');
    assert.equal(await git('status', '--porcelain'), '');
    assert.equal((await run('git', ['rev-parse', branch], upstream)).stdout.trim(), base);
    assert.deepEqual(await readdir(join(common, 'tama-ref-move-admissions')), []);
    report.managed_moves = [
      { program: subject.path, old: feature, new: stadoHead },
      { program: tama, old: stadoHead, new: current },
    ];

    // Trace2 session ancestry identifies real Git calls made by the prepared
    // hook while update-ref owns its lock, without clocks, waits or wrappers.
    const events = (await readFile(trace, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
    const starts = events.filter(event => event.event === 'start' && Array.isArray(event.argv));
    const operation = event => { const [, name] = event.argv; return name; };
    const transactions = starts.filter(event => operation(event) === 'update-ref' && event.argv.includes('--stdin'));
    assert.equal(transactions.length, report.managed_moves.length, 'each managed rewrite must reach its real admitted transaction');
    const walks = starts.filter(event => operation(event) === 'rev-list' && event.argv.includes('--remotes'));
    assert.equal(walks.length, transactions.length, 'the dropped-history review must run once per move');
    assert.deepEqual(walks.filter(event => transactions.some(transaction => event.sid.startsWith(`${transaction.sid}/`))), [], 'history was walked again inside a ref transaction');
    report.lock_observation = { transaction_sids: transactions.map(event => event.sid), dropped_history_walks: walks.map(event => ({ sid: event.sid, argv: event.argv })) };

    const unchanged = JSON.parse((await run(hook, ['move', reference, current, current, 'Confirm the current tip'])).stdout);
    assert.equal(unchanged.old, current);
    assert.equal(unchanged.new, current);
    assert.equal(await git('rev-parse', 'HEAD'), current);
    assert.deepEqual(await readdir(join(common, 'tama-ref-move-admissions')), []);

    const stale = await run(hook, ['move', reference, base, feature, 'Reject a stale move'], repo, false);
    assert.ok(exitedRefusing(stale), JSON.stringify(stale));
    assert.equal(await git('rev-parse', 'HEAD'), current);
    await writeFile(message, 'Clarify the retained value\n');
    await writeFile(plan, JSON.stringify({ commits: [{ action: 'reword', commit: current, message }] }));
    // Corrupt only this journey's generated declaration, as an older
    // installation presents it. A refusal must not retry the move via Git.
    await writeFile(hook, original.toString().split('\n').filter(line => line !== '# tama reference-move preflight').join('\n'));
    const legacy = await run(subject.path, historyArgs, repo, false);
    assert.ok(exitedRefusing(legacy), JSON.stringify(legacy));
    assert.equal(await git('rev-parse', 'HEAD'), current);
    await writeFile(hook, original);
    await run('chmod', ['a-x', hook]);
    const nonExecutable = await run(subject.path, historyArgs, repo, false);
    assert.ok(exitedRefusing(nonExecutable), JSON.stringify(nonExecutable));
    assert.equal(await git('rev-parse', 'HEAD'), current);
    await chmod(hook, originalMode);
    await run(tama, ['uninstall', '--home', home, '--git-hooks-path', hooks, '--unset-git-config', '--json']);
    const standalone = JSON.parse((await run(subject.path, historyArgs)).stdout);
    assert.equal(standalone.from, current);
    assert.equal(standalone.to, await git('rev-parse', 'HEAD'));
    assert.notEqual(standalone.to, current);
    assert.equal(await git('rev-parse', 'HEAD^{tree}'), tree);
    assert.equal(await git('status', '--porcelain'), '');
    assert.equal((await run('git', ['rev-parse', branch], upstream)).stdout.trim(), base);
    assert.equal(digest(await readFile(tama)), tamaDigest, 'the runtime executable changed during qualification');
    assert.deepEqual(await readFile(join(runtime, 'native-hook-binaries.json')), inventoryBytes);
    assert.equal(digest(await readFile(subject.path)), subject.sha256);
    report.persisted = { managed_head: current, standalone_head: standalone.to, tree, upstream: base, admissions: await readdir(join(common, 'tama-ref-move-admissions')) };
    report.outcome = 'passed';
  } catch (error) {
    report.outcome = report.dependencies_ready ? 'failed' : 'blocked';
    report.failure = { name: error.name, message: error.message, code: error.code };
    throw error;
  } finally {
    report.finished_at = new Date().toISOString();
    await writeFile(join(area, 'report.json'), JSON.stringify(report, null, '\t'));
    process.stderr.write(`History preflight evidence retained at ${area}\n`);
  }
});
