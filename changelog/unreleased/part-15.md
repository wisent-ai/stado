# Changelog Unreleased (part 15)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **A failure can name its declared cause and fix:** a command whose operation declares how it
  fails prints `fix: ...` under its `Error:` line and names the cause; `--json` answers the failure
  with `cause` and `remedy`.
- **`stado errors list|show|check`:** `list` names the operations that declare a failure catalogue,
  `show OPERATION` prints each cause with its fix, code, exit status and test (`--json`,
  `--markdown` for the documentation page), and `check` is the new `error-standard` quality gate:
  it refuses a catalogue that is not one and an error message holding a generic sentence ("did not
  succeed", "operation failed") beyond `stado-rs/errors-baseline.json`, which may only shrink.

