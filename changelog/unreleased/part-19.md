# Changelog Unreleased (part 19)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **`stado release submit`'s CLI audit no longer runs a declared launcher:** the member a release
  manifest's `runtime.launcher` names (`bin/start`) is started by release control with the binary,
  port and runtime directory it assigns, so the audit no longer reads its `--help`. Run bare,
  Skarbiec's launcher exited 1 on `SKARBIEC_BIN` and refused the release.
- **`stado release submit` carries a published diff that is not UTF-8:** a committed binary file git
  reads as text (Skarbiec's compressed inflate test vectors) put bytes that are not UTF-8 into the
  diff the source archive records for its gates, and the submission was refused with `answered
  non-UTF-8`. The record now carries exactly the bytes `git diff` gave, as `diff` when they are
  UTF-8 and as `diff_base64` when they are not, and its `sha256` is the digest of those bytes, so
  it can be checked against `git diff` itself.
