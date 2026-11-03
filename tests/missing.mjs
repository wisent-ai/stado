#!/usr/bin/env node
// Says when a committed revision lacks a construct a repair must have added,
// for a defect register's revision-bound completion probe of a capability
// (a refusal, a verb, a catalog entry) rather than of a construct that must
// go:
//
//   node tests/missing.mjs <revision> <extended-regex> <path>...
//
// Standard output carries one `missing at <revision>: <pattern> in <path>...`
// line when nothing at that revision matches, and is empty when the construct
// is present or the revision holds none of the paths (another repository's
// revision, or one from before the code existed). The revision is read in the
// repository GIT_DIR names, else in this checkout. A revision git cannot read,
// or a pattern git refuses, fails the command with git's reason.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';

// The repository the revision belongs to: the one a caller names through
// GIT_DIR (Oko runs a sweep in an export of each product's repository with
// GIT_DIR naming its object store), else this checkout. Reading this checkout
// for another repository's revision answered every product's sweep with
// Stado's history.
const root = resolve(import.meta.dirname, '..');
const repository = process.env.GIT_DIR ? ['--git-dir', process.env.GIT_DIR] : ['-C', root];
// argv holds node and this script before the caller's words.
const [, , revision, pattern, ...paths] = process.argv;
assert.ok(
  revision && pattern && paths.length,
  'usage: node tests/missing.mjs <revision> <extended-regex> <path>...',
);

const found = spawnSync(
  'git',
  [...repository, 'grep', '-q', '-E', '-e', pattern, revision, '--', ...paths],
  {
    encoding: 'utf8',
  },
);
// git grep exits zero when it matched and nonzero both when nothing matched
// and when it failed (https://git-scm.com/docs/git-grep#_exit_status); only a
// failure says why on standard error, so that complaint, a launch error or a
// signal is the failure and a quiet nonzero exit is the missing construct.
assert.ok(
  !found.error && found.signal === null && !found.stderr,
  `git grep ${pattern} at ${revision} failed (status ${found.status}, signal ${found.signal}${found.error ? `, ${found.error.message}` : ''}): ${found.stderr}`,
);
// A revision that holds none of the named paths is another repository's, or
// one from before the code existed: the construct cannot be missing from it,
// so it answers nothing. git ls-tree names each path the revision holds.
const held = spawnSync('git', [...repository, 'ls-tree', '-r', '--name-only', revision, '--', ...paths], {
  encoding: 'utf8',
});
assert.ok(
  !held.error && held.signal === null && !held.status,
  `git ls-tree ${revision} failed (status ${held.status}, signal ${held.signal}${held.error ? `, ${held.error.message}` : ''}): ${held.stderr}`,
);
if (found.status && held.stdout.trim())
  process.stdout.write(
    `missing at ${revision}: ${pattern} in ${paths.join(' ')}\n`,
  );
