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

### Fixed

- **`placement evacuate --drop-release-targets` names why a release-controlled route stays:** a
  product whose only target is the host being emptied was refused with `stado release policy
  remove` alone, although its route stayed only because `--profile-fixed-routes` could not draft
  its profile (a unit with no file, no probe), which the run kept to itself. The refusal now adds
  `or move its route: it stays on <host> because --profile-fixed-routes could not draft its
  profile: <the draft's cause>`.

- **A role move with `--with-authority` and `placement evacuate --authority` reach the
  destination's own object API at its real port:** the address was the catalog's host process
  `--bind` and `--port` taken as written, and the catalog names the port only as
  `$STADO_LISTEN_PORT`, so the check asked `http://<loopback>:$STADO_LISTEN_PORT`, was refused as
  an invalid URL, and refused every authority move. The address is now the catalog's line
  resolved for the destination: a `$STADO_LISTEN_PORT` port is the one the service directory
  records for it, refused by name when it records none, and a literal port is used as written;
  a `--port` that is no port is refused. That address becomes the route's endpoint after the
  move.

- **`stado workload start omp-session --context` publishes its instructions where the queue may
  write:** the context went under `workload-sessions/`, a prefix no object API grants, so every
  start with `--context` was refused `object_grant_does_not_cover_key`. It now goes under the
  queue's own `runs/workload-session/<run>/`, beside the inputs release runs and builds stage.
- **An installation that names no source coordinate builds the product's published revision, never its checkout's head, and never an older one than is installed:** a product installed as another's dependency (or by `stado product install|update|sync` without `--source-commit`) was built from `git rev-parse HEAD` of its canonical checkout. A checkout other sessions were working in stood on a head published history had long passed, so every install of Oko rebuilt its dependency Tama from that stale head over a newer Tama, undoing a fix each time. The unpinned revision is now the product's `origin/main`, fetched with `GIT_TERMINAL_PROMPT=0`, else the revision the delivery routine last qualified green (`~/.stado/ci-main.json`); when that is an ancestor of the installed source revision, the installed revision is built again instead. Standard error says `<product>/<surface>: no source coordinate was named, so it builds <revision>: <why>`, and `--plan` names the same revision. With neither a fetchable origin nor a qualified revision the installation refuses naming both causes and `--source-commit`.
- **`stado space reclaim` keeps the version a rollback returns to:** `delivered_trees` and the
  release trees of `delivery_leftovers` kept only what `current` names and the newest delivery,
  so a product whose `current` was its newest kept one version and an install that had to go back
  had nothing to go back to. Each version directory now also keeps the newest delivery after those.
- **`stado space reclaim` keeps every tree the host's registry service declarations name:** a
  stage kept a tree only while a process or a unit file named it, so a delivery tree the registry
  still declared as a service's program (`targets[].services[].program`), with no unit file
  naming it, was taken and the declaration was left naming nothing. Every stage now keeps a tree
  that holds, or is held by, a declared `program`, unit `path` or absolute argument, and reports it
  as `a registry service declaration names <path>; retained`; a `services` field that is not a
  list refuses the reclamation.
