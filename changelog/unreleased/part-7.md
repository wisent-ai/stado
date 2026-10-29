# Changelog Unreleased (part 7)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **`stado fleet enroll NAME --ssh DEST --install-key` enrolls a machine that has no fleet key yet:**
  Skarbiec answers the fleet's reader the same 403 for an item it may not read and for an item that
  does not exist, and enroll took that 403 as fatal. It now asks the vault owner's item list: an
  absent `stado-ssh-NAME` is minted; a present one is a mint cut off before its grant, which is
  completed and read again.
- **The `--install-key` password prompt is shown and answered:** the first-contact `ssh` ran as the
  leader of its own process group, so opening the terminal for OpenSSH's password prompt stopped it
  (SIGTTOU) and enroll waited without end. It now runs in Stado's own process group, through the
  announced wait.
- **Vault writes on another host use `skarbiec set-json`:** `stado credentials item put`, every key
  mint stored on the vault owner and the owner-vault writes called `skarbiec set`, which current
  Skarbiec no longer has, and were refused with `unknown command: set`.
- **`stado bootstrap` and `fleet enroll --bootstrap` fail when a target was not provisioned:** a
  failed install was printed as `[err]` and the command still exited 0, so enroll reported the
  machine enrolled. The run now refuses, naming every target that failed and why.
- **Bootstrap installs the Stado version the registry declares for the host:** it downloaded the
  configured `release.version` even when the host declares `managed_versions.stado`, and says which
  of the two it used.
- **A host Stado unit replaces its renamed API predecessor:** the predecessor runs `stado serve` and
  held the role lock, which refused the new unit before the API takeover that retires the
  predecessor could run, so the unit restarted beside it without end. When the role is held, the
  takeover now runs before the lock.
