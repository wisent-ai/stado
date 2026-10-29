# Changelog Unreleased (part 11)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **`stado host cargo` compiles the revision it was given:** the delivered export carries its
  commit's time on every file and the host's Cargo target is shared by every run, so a revision
  committed before that target's last compile kept the earlier revision's build of each workspace
  crate, and a check, clippy or test of it judged code it was never given. Every delivered file now
  takes the delivery's time; dependencies from registries stay compiled.
