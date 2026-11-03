// Real journey of `stado release status --run ID --wait`, through the built
// Stado, over a local store of its own (WC_STORAGE_BACKEND=local under
// .build/release-status-wait, STADO_CONFIG the empty tests/config/empty.json,
// HOME of its own). Run records are written where the release pipeline writes
// them (`runs/release-pipeline/<run>/run.json`).
//
// 1. A run still `publishing`: `--wait` says it is waiting and holds. The
//    journey reads that line on stderr, records the run `completed`, and the
//    command ends with success and prints the completed run. Nothing here
//    waits on a timer: the next step starts on the command's own words.
// 2. A run still `delivering` that is recorded `failed`: the wait ends refused
//    with `run <id> finished failed without publishing: <failure>`.
// 3. A run already `superseded`: refused at once, naming the run.
// 4. A `--run` prefix two runs share: refused with both ids, before any wait.
// 5. A run that is already `completed`: printed at once, with success.
//
// Needs STADO_BIN.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exitedCleanly, exitedRefusing } from '../../native/process.mjs';

const binary = process.env.STADO_BIN;
assert.ok(binary, 'STADO_BIN is required');
const stadoBinary = realpathSync(binary);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const output = join(root, '.build', 'release-status-wait', `run-${randomUUID()}`);
const store = join(output, 'store');
const home = join(output, 'home');
const product = `status-wait-${randomUUID()}`;
mkdirSync(home, { recursive: true });
const environment = {
  ...process.env,
  HOME: home,
  STADO_CONFIG: join(root, 'tests', 'config', 'empty.json'),
  WC_STORAGE_BACKEND: 'local',
  WC_LOCAL_STORAGE_PATH: store,
};
const report = {
  started_at: new Date().toISOString(),
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).stdout.trim(),
  stado: stadoBinary,
  stado_sha256: createHash('sha256').update(readFileSync(stadoBinary)).digest('hex'),
  commands: [], runs: [], verdict: 'failed',
};
const save = () => writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '  '));

// One run as the release pipeline records it; a caller names the id to write
// a run's next state over it, or ids that share a prefix.
function recorded(state, runId = randomUUID(), failure = undefined) {
  const path = join(store, 'runs', 'release-pipeline', runId, 'run.json');
  mkdirSync(dirname(path), { recursive: true });
  const body = { run_id: runId, product, version: '1.0.0', state, source_commit: '0'.repeat(40), platforms: {} };
  if (failure !== undefined) body.failure = failure;
  writeFileSync(path, JSON.stringify(body, null, '  '));
  report.runs.push({ path, body });
  return runId;
}

function status(runId) {
  const args = ['release', 'status', '--run', runId, '--wait', '--json'];
  const result = spawnSync(stadoBinary, args, { env: environment, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr });
  return result;
}

// Start `--wait` on a run still moving, wait for its own "waiting" line, then
// let `finish` write the run's end; resolves with the command's whole answer.
function waitThrough(runId, finish) {
  const args = ['release', 'status', '--run', runId, '--wait', '--json'];
  const child = spawn(stadoBinary, args, { env: environment });
  let stdout = '';
  let stderr = '';
  let finished = false;
  const announced = `run ${runId} is `;
  child.stdout.on('data', chunk => { stdout += chunk; });
  child.stderr.on('data', chunk => {
    stderr += chunk;
    if (!finished && stderr.includes(announced) && stderr.includes('waiting for it to finish')) {
      finished = true;
      finish();
    }
  });
  return new Promise(resolveAnswer => {
    child.on('close', (code, signal) => {
      const answer = { args, status: code, signal, stdout, stderr, released_after_wait_line: finished };
      report.commands.push(answer);
      resolveAnswer(answer);
    });
  });
}

try {
  // 1. Publishing, then completed.
  const publishing = recorded('publishing');
  const completed = await waitThrough(publishing, () => recorded('completed', publishing));
  assert.ok(completed.released_after_wait_line, `--wait never said it was waiting: ${completed.stderr}`);
  assert.ok(exitedCleanly(completed), `a run that published ended the wait refused: ${completed.stderr}`);
  const printed = JSON.parse(completed.stdout).runs;
  assert.equal(printed.length, 1, completed.stdout);
  assert.equal(printed[0].run_id, publishing);
  assert.equal(printed[0].state, 'completed');

  // 2. Delivering, then failed.
  const delivering = recorded('delivering');
  const failure = 'the darwin-arm build exited 101';
  const failed = await waitThrough(delivering, () => recorded('failed', delivering, failure));
  assert.ok(failed.released_after_wait_line, `--wait never said it was waiting: ${failed.stderr}`);
  assert.ok(exitedRefusing(failed), `a failed run ended the wait with success: ${failed.stdout}`);
  assert.ok(
    failed.stderr.includes(`run ${delivering} finished failed without publishing: ${failure}`),
    failed.stderr,
  );

  // 3. Superseded already.
  const superseded = recorded('superseded', randomUUID(), 'a newer submission replaced it');
  const replaced = status(superseded);
  assert.ok(exitedRefusing(replaced), replaced.stdout);
  assert.ok(replaced.stderr.includes(`run ${superseded} finished superseded without publishing`), replaced.stderr);

  // 4. A prefix two runs share.
  const shared = randomUUID().slice(0, 8);
  const left = recorded('publishing', `${shared}-${randomUUID()}`);
  const right = recorded('publishing', `${shared}-${randomUUID()}`);
  const ambiguous = status(shared);
  assert.ok(exitedRefusing(ambiguous), ambiguous.stdout);
  assert.ok(ambiguous.stderr.includes('--wait follows one, so name it by more of its id'), ambiguous.stderr);
  assert.ok(ambiguous.stderr.includes(left) && ambiguous.stderr.includes(right), ambiguous.stderr);

  // 5. Completed already.
  const done = recorded('completed');
  const immediate = status(done);
  assert.ok(exitedCleanly(immediate), immediate.stderr);
  assert.ok(!immediate.stderr.includes('waiting for it to finish'), immediate.stderr);
  assert.equal(JSON.parse(immediate.stdout).runs[0].run_id, done);

  report.verdict = 'passed';
  console.log(`release status --wait: passed; report ${join(output, 'report.json')}`);
} finally {
  report.finished_at = new Date().toISOString();
  save();
}
