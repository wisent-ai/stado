// Real check of the PATH an isolated-home journey hands Stado: on this
// machine's own PATH, no directory left in it resolves `stado` to the
// person's installed program, and the isolated home's bin directories lead.
// Before isolatedPath filtered by what each `stado` resolves to, a conda
// environment's bin linking ~/.stado/bin/stado stayed on PATH and every
// finish in an isolated home was refused with "would change the meaning of
// 'stado' on PATH". Reads only; nothing is written.
//
//   node tests/native/isolated_path.mjs
import assert from 'node:assert/strict';
import { existsSync, realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, join } from 'node:path';
import { isolatedPath } from './home.mjs';

const home = join(homedir(), '.build-isolated-path-journey-home');
const own = [join(homedir(), '.local/bin/stado'), join(homedir(), '.stado/bin/stado')]
  .filter(path => existsSync(path))
  .map(path => realpathSync(path));
assert.ok(own.length, 'the person has no installed stado to keep off PATH, so this machine cannot check it');

const directories = isolatedPath(home).split(delimiter);
assert.deepEqual(directories.slice(0, 2), [join(home, '.local/bin'), join(home, '.stado/bin')]);
const kept = directories
  .map(directory => join(directory, 'stado'))
  .filter(candidate => existsSync(candidate) && own.includes(realpathSync(candidate)));
assert.deepEqual(kept, [], `PATH still runs the person's stado from ${kept.join(', ')}`);
const removed = (process.env.PATH ?? '').split(delimiter).filter(directory => !directories.includes(directory));
console.log(`passed: ${directories.length} directories kept, ${removed.length} left out: ${[...new Set(removed)].join(', ')}`);
