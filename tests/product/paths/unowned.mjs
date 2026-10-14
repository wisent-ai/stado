import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, constants, mkdirSync, mkdtempSync, realpathSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real journey of `stado product paths --json`: the Stado under test runs with
// HOME set to an isolated directory under .build that holds one product
// receipt owning one program in ~/.stado/bin, a link to it in ~/.local/bin, a
// launcher put in ~/.stado/bin by hand, and a file there that is not
// executable. The answer's `unowned` names the hand-placed launcher and only
// it: the owned program, the link resolving to it and the plain file are not
// listed. Required: STADO_BIN, the built Stado under test.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const stado = process.env.STADO_BIN;
assert.ok(stado, 'STADO_BIN names the built Stado under test');
mkdirSync(join(root, '.build'), { recursive: true });
const home = mkdtempSync(join(realpathSync(join(root, '.build')), 'product-unowned-paths-'));
const stadoBin = join(home, '.stado', 'bin');
const localBin = join(home, '.local', 'bin');
for (const directory of [stadoBin, localBin, join(home, 'tmp'), join(home, '.stado', 'products', 'example')]) {
  mkdirSync(directory, { recursive: true });
}

// Owner read, write and run; group and others read and run.
const RUNNABLE = constants.S_IRWXU | constants.S_IRGRP | constants.S_IXGRP | constants.S_IROTH | constants.S_IXOTH;
function program(path) {
  writeFileSync(path, '#!/bin/sh\nexit\n');
  chmodSync(path, RUNNABLE);
}
const owned = join(stadoBin, 'example');
const launcher = join(stadoBin, 'example-run-by-hand');
program(owned);
program(launcher);
writeFileSync(join(stadoBin, 'notes.txt'), 'not a program\n');
symlinkSync(owned, join(localBin, 'example'));
writeFileSync(join(home, '.stado', 'products', 'example', 'cli.json'), JSON.stringify({
  product: 'example', surface: 'cli', status: 'installed', installed_at: '1970-01-01T00:00:00Z',
  recipe: {}, installed_paths: [owned], backups: [], host: null, source_revision: null, previous: null,
}));

const env = { PATH: process.env.PATH, HOME: home, STADO_CONFIG: join(home, 'no-config.toml'), TMPDIR: join(home, 'tmp') };
const answer = spawnSync(stado, ['product', 'paths', '--json'], { cwd: root, env, encoding: 'utf8' });
writeFileSync(join(home, 'report.json'), JSON.stringify({ exit_status: answer.status, stdout: answer.stdout, stderr: answer.stderr }));
assert.ok(!answer.status && !answer.signal && !answer.error, answer.stderr || answer.error?.message);
const report = JSON.parse(answer.stdout);
const [row, ...others] = report.unowned;
assert.deepEqual(others, [], `unowned named more than the launcher: ${JSON.stringify(report.unowned)}`);
assert.equal(row?.name, 'example-run-by-hand', JSON.stringify(report.unowned));
assert.equal(row.root, stadoBin);
assert.equal(row.resolves_to, realpathSync(launcher));
assert.ok(Array.isArray(report.collisions), 'the answer keeps its PATH collisions');
