// What every journey that runs a program with HOME set to an isolated
// directory spreads first into that program's environment.
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

import { homedir } from 'node:os';
import { join } from 'node:path';

export const pinnedGit = Object.freeze({
  GIT_CONFIG_GLOBAL: join(homedir(), '.gitconfig'),
  GIT_CONFIG_NOSYSTEM: '1',
});
