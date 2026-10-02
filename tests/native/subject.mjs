import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  accessSync, chmodSync, constants, copyFileSync, readFileSync, realpathSync, statSync,
} from 'node:fs';
import { delimiter, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';

const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

export function expectedRevision(checkoutRevision) {
  const { values } = parseArgs({ options: { 'expected-revision': { type: 'string' } } });
  const revision = values['expected-revision'] ?? checkoutRevision;
  assert.match(revision, /^[0-9a-f]{40}$/, 'expected native revision must be one complete source hash');
  return revision;
}

function executable(name) {
  if (name.includes('/')) {
    const path = realpathSync(resolve(name));
    accessSync(path, constants.X_OK);
    assert.ok(statSync(path).isFile(), `not an executable file: ${path}`);
    return path;
  }
  let inaccessible;
  // fallback-justified: these are standard PATH locations for one executable,
  // not alternative providers. Failure of the selected program is never retried elsewhere.
  for (const directory of (process.env.PATH || '').split(delimiter)) {
    const path = resolve(directory, name);
    try {
      accessSync(path, constants.X_OK);
      if (statSync(path).isFile()) return realpathSync(path);
    } catch (error) {
      if (error.code === 'EACCES') inaccessible ??= error;
      else if (error.code !== 'ENOENT' && error.code !== 'ENOTDIR') throw error;
    }
  }
  if (inaccessible) throw inaccessible;
  throw new Error(`STADO_EXECUTABLE_NOT_FOUND: no executable ${name} on PATH`);
}

export function snapshotSubject(binary, directory) {
  const source = executable(binary);
  const path = join(directory, 'stado');
  // The installed file changed between earlier runs. Execute one private snapshot
  // throughout a run; cloning the file does not build or alter the installation.
  copyFileSync(source, path, constants.COPYFILE_EXCL | constants.COPYFILE_FICLONE);
  chmodSync(path, 0o500);
  return {
    path, source_path: source, sha256: digest(readFileSync(path)),
    helper_sha256: digest(readFileSync(new URL(import.meta.url))),
  };
}

export function verifyRevision(version, expected) {
  const observed = version.match(/\(rev ([0-9a-f]{40})\)/)?.[1];
  if (observed !== expected) {
    throw Object.assign(new Error(
      `Installed Stado does not identify source ${expected}: ${version}`,
    ), { code: 'STADO_REVISION_NOT_INSTALLED', observed_revision: observed ?? null });
  }
  return observed;
}
