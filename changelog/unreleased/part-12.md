# Changelog Unreleased (part 12)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **`stado workload harness install` reaches hosts on another platform:** a harness declaration
  may name where its maker publishes builds (`releases` in `stado-rs/data/work/workloads.json`:
  the version prefix `--version` prints, the download address, the checksum list and one asset per
  release platform). A host on this machine's platform still receives this machine's copy; any
  other host fetches the build of the same version published for its platform, checks it against
  the release's SHA-256 list and its own `--version`, and only then replaces the managed program.
  `omp-session` declares OMP's builds for `darwin-arm64` and `linux-amd64`, so Linux hosts can run
  sessions. The run directory is removed on failure too.
