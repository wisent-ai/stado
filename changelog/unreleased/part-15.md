# Changelog Unreleased (part 15)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **Adding a machine to the fleet declares every way it fails:** `stado fleet enroll`, `stado
  fleet join` and `stado fleet approve` raise only declared causes (`fleet.enroll` 57, `fleet.join`
  10, `fleet.approve` 63), each naming what failed, why, the state observed and what fixes it, with
  its code and exit status: the registry read and write by the class the store answered, the
  enrollment catalog's gates with the methods the fleet does allow, the channel key, the
  `--install-key` session, each way OpenSSH ends the probe (unresolvable, refused, timed out, no
  route, key refused, host key changed), the platform, the install and its rollback, and the
  network join. A failure prints `fix: ...` under its `Error:` line and names its cause; `--json`
  answers it as JSON with `cause` and `remedy`.
- **`stado errors list|show|check`:** `list` names the operations that declare a failure catalogue,
  `show OPERATION` prints each cause with its fix, code, exit status and test (`--json`,
  `--markdown` for the documentation page), and `check` is the new `error-standard` quality gate:
  it refuses a catalogue that is not one and an error message holding a generic sentence ("did not
  succeed", "operation failed") beyond `stado-rs/errors-baseline.json`, which may only shrink.

### Changed

- **Enrollment refuses a request's form before reading anything:** a name that is not a lowercase
  target identifier, an `--ssh` destination ssh would read as an option, and a `--kind` nobody
  declares are refused with exit 2; `approve` refuses a fleet nobody declared before writing.
- **A stored join request that is not JSON is `config`, exit 1,** not a retryable outage (exit 69):
  retrying never repairs it; `stado fleet reject` and a new `join` do.
