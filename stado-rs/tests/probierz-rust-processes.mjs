// The child processes this journey runs: spawning them, killing their trees
// on the way out, and turning a finished run into a record.
//
// Split out of probierz-rust-journey.mjs, which had grown past the 300-line
// file limit.
import { spawn } from 'node:child_process';

export const compilationBudgetMs = 70 * 60 * 1000;
export const executionBudgetMs = 10 * 60 * 1000;
const killGraceMs = 2 * 1000;
export const processOutputEncoding = 'utf8';
export const testArgs = ['--ignored', '--nocapture', '--test-threads=1'];
export const profileEnvironment = {
  CARGO_PROFILE_TEST_DEBUG: '0',
  CARGO_INCREMENTAL: '0',
};
const activeChildren = new Set();

export function errorRecord(error) {
  if (!error) return null;
  return {
    name: error.name || null,
    message: error.message || String(error),
    code: error.code ?? null,
    errno: error.errno ?? null,
    syscall: error.syscall ?? null,
    path: error.path ?? null,
  };
}

function signalProcessTree(child, signal) {
  if (!child.pid) return null;
  try {
    if (process.platform === 'win32') child.kill(signal);
    else process.kill(-child.pid, signal);
    return null;
  } catch (error) {
    return error.code === 'ESRCH' ? null : error;
  }
}

export function terminateActiveChildren(signal) {
  for (const child of activeChildren) signalProcessTree(child, signal);
}

for (const [signal, exitCode] of [['SIGINT', 130], ['SIGTERM', 143], ['SIGHUP', 129]]) {
  process.once(signal, () => {
    terminateActiveChildren('SIGTERM');
    setTimeout(() => {
      terminateActiveChildren('SIGKILL');
      process.exit(exitCode);
    }, killGraceMs);
  });
}

export function runProcess(executable, args, { cwd, env, timeoutMs }) {
  return new Promise((complete) => {
    const startedAt = new Date().toISOString();
    const startedMs = Date.now();
    const child = spawn(executable, args, {
      cwd,
      env,
      detached: process.platform !== 'win32',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    activeChildren.add(child);
    child.stdout.setEncoding(processOutputEncoding);
    child.stderr.setEncoding(processOutputEncoding);

    let stdout = '';
    let stderr = '';
    let spawnError = null;
    let closeResult = null;
    let timedOut = false;
    let hardKillSent = false;
    let settled = false;
    let hardKillTimer = null;

    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', (error) => { spawnError = error; });

    const finish = () => {
      if (settled || closeResult === null) return;
      if (timedOut && !hardKillSent) return;
      settled = true;
      activeChildren.delete(child);
      clearTimeout(timeoutTimer);
      clearTimeout(hardKillTimer);
      complete({
        executable,
        args,
        cwd,
        startedAt,
        completedAt: new Date().toISOString(),
        durationMs: Date.now() - startedMs,
        timeoutMs,
        exitCode: closeResult.exitCode,
        signal: closeResult.signal,
        timedOut,
        killed: Boolean(child.killed || timedOut),
        spawnError,
        stdout,
        stderr,
      });
    };

    child.on('close', (exitCode, signal) => {
      closeResult = { exitCode, signal };
      finish();
    });

    const timeoutTimer = setTimeout(() => {
      timedOut = true;
      spawnError ||= signalProcessTree(child, 'SIGTERM');
      hardKillTimer = setTimeout(() => {
        spawnError ||= signalProcessTree(child, 'SIGKILL');
        hardKillSent = true;
        finish();
      }, killGraceMs);
    }, timeoutMs);
  });
}

export function succeeded(result) {
  return !result.spawnError && !result.timedOut && result.exitCode === 0;
}

export function digestText(text) {
  return createHash('sha256').update(text).digest('hex');
}

async function digestFile(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

async function writeProcessRecord(result, artifacts, stem) {
  const stdoutPath = join(artifacts, `${stem}.stdout.log`);
  const stderrPath = join(artifacts, `${stem}.stderr.log`);
  await Promise.all([
    writeFile(stdoutPath, result.stdout, { mode: 0o600 }),
    writeFile(stderrPath, result.stderr, { mode: 0o600 }),
  ]);
  return {
    executable: result.executable,
    args: result.args,
    cwd: result.cwd,
    startedAt: result.startedAt,
    completedAt: result.completedAt,
    durationMs: result.durationMs,
    timeoutMs: result.timeoutMs,
    exitCode: result.exitCode,
    signal: result.signal,
    timedOut: result.timedOut,
    killed: result.killed,
    spawnError: errorRecord(result.spawnError),
    stdout: {
      file: stdoutPath,
      bytes: Buffer.byteLength(result.stdout),
      sha256: digestText(result.stdout),
    },
    stderr: {
      file: stderrPath,
      bytes: Buffer.byteLength(result.stderr),
      sha256: digestText(result.stderr),
    },
  };
}

async function sourceIdentity(checkpoint) {
  const options = { cwd: repository, env: process.env, timeoutMs: 30 * 1000 };
  const [revision, status] = await Promise.all([
    runProcess('git', ['rev-parse', 'HEAD'], options),
    runProcess('git', ['status', '--porcelain', '--untracked-files=all'], options),
  ]);
  if (!succeeded(revision)) {
    throw new Error(`cannot read source revision at ${checkpoint}: ${revision.stderr || revision.spawnError?.message || revision.signal || revision.exitCode}`);
  }
  if (!succeeded(status)) {
    throw new Error(`cannot read source status at ${checkpoint}: ${status.stderr || status.spawnError?.message || status.signal || status.exitCode}`);
  }
  return {
    checkpoint,
    observedAt: new Date().toISOString(),
    revision: revision.stdout.trim(),
    clean: status.stdout.trim().length === 0,
  };
}

