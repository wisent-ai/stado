// What every journey that runs a program with HOME set to an isolated
// directory spreads into that program's environment.
//
// Git reads its global configuration from $HOME/.gitconfig. Under an
// isolated HOME it found none, fell back to the system file's
// `credential.helper osxkeychain`, and every fetch Stado ran then asked
// macOS to store the GitHub token in a login keychain that does not exist
// under that HOME: the person saw "Keychain Not Found ... Reset To Defaults"
// dialogs, one per fetch, until the journey ended. GIT_CONFIG_GLOBAL names
// the ~/.gitconfig of the HOME the journey itself was started with (its
// `gh auth git-credential` helper answers), and GIT_CONFIG_NOSYSTEM drops the
// system file, so no git a journey starts can reach the keychain helper.
//
// Spread first, so a journey that names its own fixture GIT_CONFIG_GLOBAL
// keeps it; GIT_CONFIG_NOSYSTEM then still keeps the system helper away.

import { existsSync, realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, join } from 'node:path';

export const pinnedGit = Object.freeze({
  GIT_CONFIG_GLOBAL: join(homedir(), '.gitconfig'),
  GIT_CONFIG_NOSYSTEM: '1',
});

// The programs an installation into an isolated home places under one name
// the person's own installation already answers to.
const SHADOWED = ['stado'];

// Whether `directory` holds one of SHADOWED that is the person's own program,
// by the file it resolves to: a link elsewhere on PATH (a conda environment's
// bin linking ~/.stado/bin/stado) is the person's program as much as
// ~/.local/bin/stado is.
function personal(directory, own) {
  return SHADOWED.some(name => {
    const candidate = join(directory, name);
    if (!existsSync(candidate)) return false;
    return own.has(realpathSync(candidate));
  });
}

// The PATH a Stado that installs into the isolated `home` runs under: that
// home's ~/.local/bin and ~/.stado/bin first, and every directory whose
// `stado` is the person's own left out. An installation's finish refuses to
// place a program that would change what a name on PATH runs, so with the
// person's ~/.local/bin, or any link to its program, anywhere on PATH every
// finish in an isolated home was refused with "would change the meaning of
// 'stado' on PATH; it already runs <that path>".
export function isolatedPath(home, inherited = process.env.PATH ?? '') {
  const own = new Set(
    [join(homedir(), '.local/bin'), join(homedir(), '.stado/bin')]
      .flatMap(directory => SHADOWED.map(name => join(directory, name)))
      .filter(path => existsSync(path))
      .map(path => realpathSync(path)),
  );
  const rest = inherited
    .split(delimiter)
    .filter(directory => directory && !personal(directory, own));
  return [join(home, '.local/bin'), join(home, '.stado/bin'), ...rest].join(delimiter);
}
