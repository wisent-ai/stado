// Running one child process for a recorded journey and stopping it the way
// the journey promises: a budget, a signal to the whole tree, a grace period,
// and a sentence saying which of those ended it.

import { spawn } from 'node:child_process';

const killGraceMs = 2 * 1000;
const processOutputEncoding = 'utf8';
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

function terminateActiveChildren(signal) {
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

export function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function formatFailure(result) {
  if (!result) return 'not run';
  if (result.spawnError) return result.spawnError.message;
  if (result.timedOut) return `timed out after ${result.timeoutMs}ms`;
  return `exit ${result.exitCode ?? 'unknown'}${result.signal ? ` (${result.signal})` : ''}`;
}
