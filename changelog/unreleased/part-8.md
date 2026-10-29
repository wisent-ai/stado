# Changelog Unreleased (part 8)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **`stado host cargo TARGET build|check|clippy|test --manifest-path PATH --revision REV [--bin NAME]… [--json] [-- ARGS]`**
  runs one Cargo operation on a committed revision on a fleet host in one command. The revision is
  exported from its canonical checkout, delivered into a fresh managed run on the host and run
  there `--locked` with the revision's own `rust-toolchain.toml`, the declared compiler cache and
  the host's shared Cargo target in its build cache (below the registry's `work_root` when the
  host declares one). `build` is `--release` and brings each `--bin` back to
  `<manifest directory>/target/host-cargo/<commit>/`. The run is removed whatever Cargo answered,
  and the exit status is Cargo's. The receipt holds the commit, Cargo's streams, the binaries with
  their SHA-256, and the free space the run took.

### Removed

- `stado host run build`: `stado host cargo … build` replaces it. The old verb compiled with the
  host's default rustup toolchain, because rustup reads the toolchain from the directory it starts
  in and never from `--manifest-path`. It also left the export, delivery and cleanup to the caller.
