# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.69 – 0.23.70](changelog/0.23.69-0.23.70.md)
- [0.23.68 (part 1 of 10)](changelog/0.23.68-part-1.md)
- [0.23.68 (part 2 of 10)](changelog/0.23.68-part-2.md)
- [0.23.68 (part 3 of 10)](changelog/0.23.68-part-3.md)
- [0.23.68 (part 4 of 10)](changelog/0.23.68-part-4.md)
- [0.23.68 (part 5 of 10)](changelog/0.23.68-part-5.md)
- [0.23.68 (part 6 of 10)](changelog/0.23.68-part-6.md)
- [0.23.68 (part 7 of 10)](changelog/0.23.68-part-7.md)
- [0.23.68 (part 8 of 10)](changelog/0.23.68-part-8.md)
- [0.23.68 (part 9 of 10)](changelog/0.23.68-part-9.md)
- [0.23.68 (part 10 of 10)](changelog/0.23.68-part-10.md)
- [0.23.67](changelog/0.23.67.md)
- [0.23.66 (part 1 of 2)](changelog/0.23.66-part-1.md)
- [0.23.66 (part 2 of 2)](changelog/0.23.66-part-2.md)
- [0.23.42 – 0.23.65](changelog/0.23.42-0.23.65.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- [Unreleased (part 1)](changelog/unreleased/part-1.md)

### Added

- **`stado change status`, `stado change list`, a landing's integration line, its risk and its
  review name every gate the operator's rulings left out of a submit-queue pass:** the pass
  printed the ruling to its error output only, so a candidate could land on the source check
  alone while its record read as gated. Each gate left out is a shard with exit `-1`, host `none`
  and `left_out` holding Oko's words; status prints `gate <name> (<kind>) for <platform>: not
  run, left out: <words>`, list adds `; gates left out: …`, the integration line says
  `gates: not-run`, the risk is classified again from the gates that ran, and the review is
  held for a recorded round whose wait names the gates that never ran. Journey
  `tests/change/left_out/`.

### Fixed

- **A janitor pass that meets running workloads' recorded holds still runs the cleaners no
  running workload needs kept whole:** it returned before any cleaner after the lock ran, so
  while release jobs compiled the volume filled with almost nothing freed. Beside the holds it now
  runs `bin_backups` and `delivered_releases`, whose keep rules protect what a running workload
  uses, and at the rule `local_snapshots`, Time Machine's snapshots, which no job reads, under its
  own shared hold and the new `disk-cleanup-unheld.lock`, records their removals, and every pass
  that finds the lock held names each cleaner after the lock it did not run in
  `waited_cleaners`, shown by `stado space report`, `stado space reclaim TARGET`, the public
  beacon, and Stado Desktop's Disk screen and Space section. Journey
  `tests/space_reclaim/guards/workload_hold.rs`.

- **The release gates run the memory verdict's journey:** `stado host processes` naming the
  process that runs a host out of memory before the next reading was proven only by a journey
  compiled into the `scheduler` test binary, which no release gate ran, so no release recorded
  it passing. The darwin-arm64 and linux-amd64 gate `host-processes-memory` runs
  `tests/host_processes/memory.rs`.

- **Stado compiles again, and the submit queue compiles the whole workspace for linux-amd64
  before a candidate lands:** the product CLI re-exported `route::run` twice (`E0252`), and the
  workspace origin scan applied `?` to the git runner's error inside a function that returns an
  `io::Error` (`E0277`), so no build of `main` passed on any platform and no Linux host could be
  upgraded. The second re-export is gone, and the scan carries the runner's refusal whole inside
  the `io::Error` its wait records. `.wisent-release.json` gives linux-amd64 the quality gate
  `workspace-compiles` (`cargo test --locked --manifest-path stado-rs/Cargo.toml --workspace
  --no-run`): `stado change run` runs it on a linux-amd64 fleet host for every candidate whose
  test gates Oko admits, and the release build runs it before building, so a revision whose
  library, binaries, tests or examples do not compile for Linux is refused with the compiler's
  report instead of landing. Where the operator's rulings leave test gates out, `stado change
  status` names this gate as left out and the candidate waits for its review round.
- **The stado library compiles again after the fleet database re-address:** `stado database tls
  reissue` and `stado database migrate`'s reuse of a pending fleet server read the item's port
  through a match whose refusing arm named the conversion error in one alternative of an
  or-pattern and not in the other (`E0408`), so no revision since then built on any platform. The
  arm binds nothing, and an item whose port is missing or wider than a TCP port is still refused
  as naming no TCP port.
