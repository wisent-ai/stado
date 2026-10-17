import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { snapshotSubject, verifyRevision } from '../native/subject.mjs';

// Real `stado sandbox` journey against Boat Public API v1.
// 1. On an isolated STADO_CONFIG naming the boat provider without its
//    settings, and with the BOAT_* overrides removed, `stado sandbox list`
//    refuses before any request, naming boat.ttl_seconds.
// 2. With the operator's configuration (boat settings and the Skarbiec item
//    tagged stado:role:cloud-boat), STADO_BOAT_TEST_TTL_SECONDS is the
//    lifetime of the sandboxes this journey creates. It creates a sandbox,
//    refuses a lifetime that is not positive, writes a file with a fresh
//    token and reads it back, runs a command that prints it, sees a failing
//    command fail the CLI with its exit, downloads the file and is refused
//    a second download over it, renames the sandbox and reads the name back,
//    reads its events, forks the sandbox and reads the token on the fork,
//    deletes the fork, stops the original and deletes it. Prompting the
//    coding agent needs an agent provider's account and is not run here.
//    The last `show` of each sandbox is kept as the observed final state.
// STADO_BIN selects the Stado under test. Without STADO_BIN or
// STADO_BOAT_TEST_TTL_SECONDS the journey is blocked, never passed. The
// report keeps every command, exit status and output under .build/; a
// failure is rethrown after the report is written.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'providers-boat-sandbox-'));
const stado = process.env.STADO_BIN;
const lifetime = process.env.STADO_BOAT_TEST_TTL_SECONDS;
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
// A clean exit: no spawn error, no signal, and a status that is zero (falsy).
const exitedCleanly = answer => !answer.error && !answer.signal && !answer.status;
function run(program, args, env = process.env) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8' });
  report.commands.push({ program, arguments: args, config: env.STADO_CONFIG, exit_status: answer.status,
    signal: answer.signal, error: answer.error?.message, stdout: answer.stdout, stderr: answer.stderr });
  return answer;
}
function success(answer) {
  assert.ok(exitedCleanly(answer), JSON.stringify({ stderr: answer.stderr, error: answer.error?.message,
    signal: answer.signal, status: answer.status }));
  return answer.stdout.trim();
}
function refused(answer, words) {
  assert.ok(!exitedCleanly(answer), `accepted: ${answer.stdout}`);
  assert.ok(`${answer.stderr}${answer.stdout}`.includes(words), `the refusal does not name ${words}: ${answer.stderr}`);
}
function withoutBoatOverrides(document) {
  const path = join(output, 'unset.json');
  writeFileSync(path, `${JSON.stringify(document)}\n`);
  const env = { ...process.env, STADO_CONFIG: path };
  for (const name of Object.keys(env)) {
    if (name.startsWith('BOAT_')) delete env[name];
  }
  return env;
}
try {
  report.source_revision = success(run('git', ['rev-parse', 'HEAD']));
  report.test_sha256 = digest(readFileSync(fileURLToPath(import.meta.url)));
  if (!stado || !lifetime) {
    report.result = 'blocked';
    throw new Error(`The Boat sandbox journey requires STADO_BIN and STADO_BOAT_TEST_TTL_SECONDS; missing: ${[
      !stado && 'STADO_BIN', !lifetime && 'STADO_BOAT_TEST_TTL_SECONDS',
    ].filter(Boolean).join(', ')}`);
  }
  assert.equal(success(run('git', ['status', '--porcelain', '--untracked-files=all'])), '',
    'the Boat sandbox journey requires a clean committed source tree, including no untracked files');
  report.stado = snapshotSubject(stado, output);
  const binary = report.stado.path;
  report.stado.version = success(run(binary, ['--version']));
  verifyRevision(report.stado.version, report.source_revision);

  refused(run(binary, ['sandbox', 'list'], withoutBoatOverrides({ providers: ['boat'] })), 'boat.ttl_seconds');

  const sandbox = args => JSON.parse(success(run(binary, ['sandbox', ...args])));
  refused(run(binary, ['sandbox', 'create', `--ttl-seconds=-${lifetime}`]), 'is not a positive number of seconds');
  const original = sandbox(['create', '--ttl-seconds', lifetime]);
  assert.ok(original.id, `create answered no sandbox id: ${JSON.stringify(original)}`);
  report.original = original.id;

  const token = randomUUID();
  const local = join(output, 'token.txt');
  writeFileSync(local, token);
  const file = `stado-sandbox-${token}.txt`;
  sandbox(['write', original.id, file, '--from', local, '--encoding', 'utf8']);
  const read = sandbox(['read', original.id, file, '--encoding', 'utf8']);
  assert.ok(JSON.stringify(read).includes(token), `reading ${file} did not answer the written token: ${JSON.stringify(read)}`);

  const printed = sandbox(['exec', original.id, `cat ${file}`]);
  assert.equal(printed.success, true, `cat did not succeed: ${JSON.stringify(printed)}`);
  assert.ok(printed.stdout.includes(token), `cat did not print the token: ${JSON.stringify(printed)}`);
  refused(run(binary, ['sandbox', 'exec', original.id, 'false']), `the command in sandbox ${original.id} exited with`);
  refused(run(binary, ['sandbox', 'exec', original.id, '--cwd', '', 'true']), '--cwd names no directory');

  const downloaded = join(output, 'downloaded.bin');
  sandbox(['download', original.id, file, '--to', downloaded]);
  assert.ok(readFileSync(downloaded).includes(token), `downloading ${file} did not bring the token back`);
  refused(run(binary, ['sandbox', 'download', original.id, file, '--to', downloaded]), `to ${downloaded}`);

  const named = `stado-sandbox-${token}`;
  assert.equal(sandbox(['update', original.id, '--name', named]).name, named, 'update did not rename the sandbox');
  assert.equal(sandbox(['show', original.id]).name, named, 'show does not read the new name back');

  const events = sandbox(['events', original.id, '--sort', 'asc']);
  assert.ok(Array.isArray(events.events), `events answered no list: ${JSON.stringify(events)}`);
  assert.ok(events.events.length, `the sandbox that ran commands has no events: ${JSON.stringify(events)}`);

  const fork = sandbox(['fork', original.id]);
  assert.ok(fork.id && fork.id !== original.id, `fork answered no new sandbox: ${JSON.stringify(fork)}`);
  report.fork = fork.id;
  const copied = sandbox(['exec', fork.id, `cat ${file}`]);
  assert.ok(copied.stdout.includes(token), `the fork does not hold the original's file: ${JSON.stringify(copied)}`);
  assert.deepEqual(sandbox(['delete', fork.id]), { deleted: fork.id });

  sandbox(['stop', original.id]);
  report.stopped = sandbox(['show', original.id]);
  assert.deepEqual(sandbox(['delete', original.id]), { deleted: original.id });
  report.final = {
    original: run(binary, ['sandbox', 'show', original.id]),
    fork: run(binary, ['sandbox', 'show', fork.id]),
  };
  report.result = 'passed';
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  report.finished_at = new Date().toISOString();
  report.scope = 'Boat sandbox CLI: configuration refusal, and the full lifecycle of a real sandbox and its fork';
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
  console.log(`${report.result}: ${join(output, 'report.json')}`);
}
