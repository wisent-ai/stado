// What a journey keeps as proof: the digest of every stream it captured, the
// files those streams were written to, and the exact source revision the run
// was bound to.

import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { runProcess, succeeded } from './process.mjs';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

export function digestText(text) {
  return createHash('sha256').update(text).digest('hex');
}

export async function digestFile(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

export async function writeProcessRecord(result, artifacts, stem) {
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

export async function sourceIdentity(checkpoint) {
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
