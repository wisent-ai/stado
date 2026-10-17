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
//    token and reads it back, is refused a recover of the sandbox that just
//    answered (sandbox_is_reachable), runs a command that prints it, sees a failing
//    command fail the CLI with its exit, starts a command in the background,
//    finds it running and stops it, keeps the sandbox's usage, reads its
//    desktop URL, downloads the file and is refused a second download over
//    it, renames the sandbox and reads the name back, reads its events,
//    creates an environment, sets a variable in it from standard input (an
//    empty one is refused), starts a sandbox with it that prints the
//    variable, deletes that sandbox, removes the variable and deletes the
//    environment, gives STADO_BOAT_TEST_PORT a public URL, saves the
//    disk as a named snapshot, finds it listed and shown, deletes it and no
//    longer finds it listed, forks the sandbox and reads the token on the
//    fork, deletes the fork, stops the original, finds the file in its latest
//    snapshot's tree and takes it back out of that snapshot with the token,
//    and deletes it. Prompting the coding agent (and steering it) needs an
//    agent provider's account and is not run here.
//    The last `show` of each sandbox is kept as the observed final state.
// STADO_BIN selects the Stado under test. Without STADO_BIN,
// STADO_BOAT_TEST_TTL_SECONDS or STADO_BOAT_TEST_PORT the journey is blocked,
// never passed. The report keeps every command, exit status and output under
// .build/; a failure is rethrown after the report is written.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
mkdirSync(join(root, '.build'), { recursive: true });
const output = mkdtempSync(join(realpathSync(join(root, '.build')), 'providers-boat-sandbox-'));
const stado = process.env.STADO_BIN;
const lifetime = process.env.STADO_BOAT_TEST_TTL_SECONDS;
const port = process.env.STADO_BOAT_TEST_PORT;
const report = { started_at: new Date().toISOString(), commands: [], result: 'failed' };
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
// A clean exit: no spawn error, no signal, and a status that is zero (falsy).
const exitedCleanly = answer => !answer.error && !answer.signal && !answer.status;
function run(program, args, env = process.env, input = undefined) {
  const answer = spawnSync(program, args, { cwd: root, env, encoding: 'utf8', input });
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
  if (!stado || !lifetime || !port) {
    report.result = 'blocked';
    throw new Error(`The Boat sandbox journey requires STADO_BIN, STADO_BOAT_TEST_TTL_SECONDS and STADO_BOAT_TEST_PORT; missing: ${[
      !stado && 'STADO_BIN', !lifetime && 'STADO_BOAT_TEST_TTL_SECONDS', !port && 'STADO_BOAT_TEST_PORT',
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
  // A sandbox that just answered a read is reachable, so Boat refuses to
  // power-cycle it and the CLI keeps Boat's code.
  refused(run(binary, ['sandbox', 'recover', original.id]), 'sandbox_is_reachable');

  const printed = sandbox(['exec', original.id, `cat ${file}`]);
  assert.equal(printed.success, true, `cat did not succeed: ${JSON.stringify(printed)}`);
  assert.ok(printed.stdout.includes(token), `cat did not print the token: ${JSON.stringify(printed)}`);
  refused(run(binary, ['sandbox', 'exec', original.id, 'false']), `the command in sandbox ${original.id} exited with`);
  refused(run(binary, ['sandbox', 'exec', original.id, '--cwd', '', 'true']), '--cwd names no directory');

  const started = sandbox(['exec', original.id, '--detach', 'tail -f /dev/null']);
  assert.ok(Number.isSafeInteger(started.processId), `exec --detach answered no process id: ${JSON.stringify(started)}`);
  const background = String(started.processId);
  const status = sandbox(['process', 'status', original.id, background]);
  assert.equal(status.processId, started.processId, `process status answered another process: ${JSON.stringify(status)}`);
  assert.equal(status.running, true, `the background command is not running: ${JSON.stringify(status)}`);
  sandbox(['process', 'stop', original.id, background]);
  report.stopped_process = sandbox(['process', 'status', original.id, background]);
  report.usage = sandbox(['usage', original.id]);
  const desktop = sandbox(['desktop', original.id]);
  assert.ok(desktop.desktopUrl === null || String(desktop.desktopUrl).startsWith('https://'),
    `desktop answered a URL that is not HTTPS: ${JSON.stringify(desktop.mode)}`);

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

  const environmentName = `stado-sandbox-${token}`;
  const created = sandbox(['environment', 'create', environmentName]);
  const environment = created.environments.find(row => row.name === environmentName);
  assert.ok(environment?.id, `environment create did not list ${environmentName}: ${JSON.stringify(created)}`);
  success(run(binary, ['sandbox', 'environment', 'set-var', environment.id, 'STADO_SANDBOX_TOKEN'], process.env, `${token}\n`));
  refused(run(binary, ['sandbox', 'environment', 'set-var', environment.id, 'STADO_SANDBOX_TOKEN'], process.env, ''),
    'standard input holds no value for STADO_SANDBOX_TOKEN');
  const configured = sandbox(['create', '--ttl-seconds', lifetime, '--environment', environment.id]);
  const printedVariable = sandbox(['exec', configured.id, 'printenv STADO_SANDBOX_TOKEN']);
  assert.ok(printedVariable.stdout.includes(token), `a sandbox of ${environment.id} does not hold its variable`);
  assert.deepEqual(sandbox(['delete', configured.id]), { deleted: configured.id });
  sandbox(['environment', 'rm-var', environment.id, 'STADO_SANDBOX_TOKEN']);
  sandbox(['environment', 'delete', environment.id]);
  assert.ok(!sandbox(['environment', 'list']).environments.some(row => row.id === environment.id),
    `environment ${environment.id} is still listed after its deletion`);

  const hosted = sandbox(['host', original.id, port]);
  assert.ok(String(hosted.url).startsWith('https://'), `host answered no HTTPS URL: ${JSON.stringify(hosted.port)}`);
  assert.equal(String(hosted.port), port, 'host answered another port than the one asked for');

  const saved = sandbox(['snapshot', 'save', original.id, named]);
  assert.equal(saved.snapshot?.name, named, `snapshot save answered another name: ${JSON.stringify(saved)}`);
  const listed = sandbox(['snapshot', 'list']);
  assert.ok(listed.snapshots.some(row => row.name === named), `snapshot list does not hold ${named}`);
  assert.equal(sandbox(['snapshot', 'show', named]).snapshot?.name, named, `snapshot show does not read ${named}`);
  sandbox(['snapshot', 'delete', named]);
  report.snapshot_after_delete = run(binary, ['sandbox', 'snapshot', 'show', named]);
  assert.ok(!sandbox(['snapshot', 'list']).snapshots.some(row => row.name === named),
    `snapshot list still holds ${named} after its deletion`);

  const fork = sandbox(['fork', original.id]);
  assert.ok(fork.id && fork.id !== original.id, `fork answered no new sandbox: ${JSON.stringify(fork)}`);
  report.fork = fork.id;
  const copied = sandbox(['exec', fork.id, `cat ${file}`]);
  assert.ok(copied.stdout.includes(token), `the fork does not hold the original's file: ${JSON.stringify(copied)}`);
  assert.deepEqual(sandbox(['delete', fork.id]), { deleted: fork.id });

  sandbox(['stop', original.id]);
  report.stopped = sandbox(['show', original.id]);
  report.snapshots = sandbox(['snapshot', 'history', original.id]);
  const latest = sandbox(['snapshot', 'latest', original.id]).snapshot;
  assert.ok(latest?.id, `the stopped sandbox has no latest snapshot: ${JSON.stringify(latest)}`);
  const tree = sandbox(['snapshot', 'tree', latest.id]);
  assert.ok(tree.entries.some(entry => entry.path.endsWith(file)), `snapshot ${latest.id} does not list ${file}`);
  const restored = join(output, 'restored.bin');
  sandbox(['snapshot', 'get', latest.id, file, '--to', restored]);
  assert.ok(readFileSync(restored).includes(token), `the file taken out of snapshot ${latest.id} lost the token`);
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
