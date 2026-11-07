// How a journey reads the end of a process it ran with `spawnSync`, or an
// exit code a receipt recorded. The success status is the platform's
// EXIT_SUCCESS, which `spawnSync` reports as a falsy integer status; it is
// read here, once, so no journey has to be told it by its caller. A process
// the journey asks about while it still runs is started with `started`.
import { spawn } from 'node:child_process';
import { watch } from 'node:fs';

/** A recorded exit code that is the platform's success. */
export function succeededStatus(code) {
  return Number.isInteger(code) && !code;
}

/** Started, not ended by a signal, and exited with success. */
export function exitedCleanly(answer) {
  return !answer.error && !answer.signal && succeededStatus(answer.status);
}

/** Started, not ended by a signal, and exited with a failure status of its own. */
export function exitedRefusing(answer) {
  return !answer.error && !answer.signal && Number.isInteger(answer.status)
    && !succeededStatus(answer.status);
}

/**
 * Start `program` without waiting for it. `child` is the running process;
 * `ended` settles once it has ended, with the fields `spawnSync` answers
 * (`status`, `signal`, `stdout`, `stderr`, `error`), so the same readers
 * judge it.
 */
export function started(program, args, options) {
  const child = spawn(program, args, { ...options, stdio: ['ignore', 'pipe', 'pipe'] });
  const printed = { stdout: [], stderr: [] };
  child.stdout.on('data', chunk => printed.stdout.push(chunk));
  child.stderr.on('data', chunk => printed.stderr.push(chunk));
  const text = stream => Buffer.concat(printed[stream]).toString('utf8');
  const ended = new Promise(settle => {
    child.on('error', error => settle({ error, status: null, signal: null,
      stdout: text('stdout'), stderr: text('stderr') }));
    child.on('close', (status, signal) => settle({ status, signal,
      stdout: text('stdout'), stderr: text('stderr') }));
  });
  return { child, ended };
}

/**
 * Settles with `null` once `holds()` is true, asked at once and again on
 * every change the system reports in `directory`, so nothing waits on a
 * clock; settles with `ended`'s answer instead when the process that would
 * make it true ends first. The directory is watched no longer either way.
 */
export function holdsBefore(directory, holds, ended) {
  return new Promise(settle => {
    const watcher = watch(directory);
    const ask = () => {
      if (holds()) {
        watcher.close();
        settle(null);
      }
    };
    watcher.on('change', ask);
    ended.then(answer => {
      watcher.close();
      settle(answer);
    });
    ask();
  });
}
