// How a journey reads the end of a process it ran with `spawnSync`, or an
// exit code a receipt recorded. The success status is the platform's
// EXIT_SUCCESS, which `spawnSync` reports as a falsy integer status; it is
// read here, once, so no journey has to be told it by its caller.

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
