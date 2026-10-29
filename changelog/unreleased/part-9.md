# Changelog Unreleased (part 9)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **Stado no longer asks the person at a Mac about Apple Music, Photos, Contacts or Files and
  Folders it does not need:** the janitor's build-cache walk descended `~/Library`, where it opened
  the Photos syndication library and the media library, and the always-on host process made macOS
  ask for photos and the media library; every beacon opened `~/Documents`, `~/Desktop` and
  `~/Downloads` to measure them; the janitor opened all three each pass to decide where Cargo
  builds; and `stado space report` ran one `du` over the whole home. Every walk of a home now
  refuses one list — `Pictures`, `Music`, `Movies`, `Library`, `.Trash`, and each of `Documents`,
  `Desktop` and `Downloads` that `targets[].privacy_grants` does not declare for the program macOS
  holds responsible for the walk (the running Stado here, `/usr/libexec/sshd-keygen-wrapper` for a
  script walk over SSH on another Mac) —
  and nothing is opened to find out whether macOS allows it. The beacon measures only declared
  folders (`not measured` for the rest), and the `du` inventory walks the home one top-level
  entry at a time so a refused one is never opened; `~/Library/Caches` is no longer an inventory
  root.

### Added

- **`stado host privacy TARGET --since WINDOW`** lists every privacy request macOS's `tccd`
  recorded on TARGET in the window against its Stado program and the programs it started — time,
  service, check or access, whether a dialog was shown, the answer, the accessing program and who
  asked — read from TARGET's unified log through its declared connection, and exits non-zero when
  one reached for protected data no grant declares (`UNDECLARED`). `--json` carries `requests`
  and `undeclared_requests`.

### Changed

- **`stado release local install` and `release local restore` refuse a program without a stable
  signing identity:** macOS keeps every privacy decision per signing identity, so an ad-hoc signed
  build placed in `~/.stado/bin` was a new program to it and asked every question again. A native
  program that is not Developer ID or Apple Development signed is refused, naming its signature
  state and `stado product install NAME --source-commit <commit on origin/main>`, and the
  installed program stays.
