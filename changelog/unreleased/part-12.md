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
- **The registry records which bearers a host holds (`targets[].bearers`):** every bearer file
  `stado credentials token sync` delivers and every owner file `stado credentials token mint
  --token-file-name` writes is recorded on its host as `{consumer, file}`, so
  `stado registry host show HOST` says which of the fleet's credentials sit on a shared computer.
- **`stado credentials token remove CONSUMER --host HOST --token-file FILE`:** takes a bearer file
  off a host and its record out of the registry, the counterpart of `sync`. It is refused while
  the vault owner still records a grant for CONSUMER, while the registry records the file as
  another consumer's, and while the host's configuration reads a bearer from it; a symbolic link,
  a file another account owns and a path outside the account's home are never removed. A file
  already absent is reported as `token_absent`.
