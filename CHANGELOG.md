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

- **`stado host inventory TARGET` on a Linux host dates a service artefact and its installed
  program, and its answer parses:** the inventory read both ages with the BSD `stat -f %m` on every
  kernel; GNU stat read `-f` as file-system status, printed the file system's multi-line status
  and failed, and that text went unchecked into the report's JSON, so a Linux host with a service
  artefact answered an inventory no reader could parse. Both ages are now read by the one
  modification-time reader the reclaim program's second look already used, now a fragment both
  remote programs splice, in the kernel's own `stat` dialect and kept only when they are an
  integer; otherwise the epoch is empty and `artefact_epoch_error` or `installed_epoch_error`
  carries stat's answer, which the fleet shape check's note names. Journey
  `tests/host_inventory/artefact_epochs.rs`, run on darwin-arm64 and linux-amd64 by the release
  gate `inventory-artefact-epochs`.

- **`stado space scratch register` refuses an owner PID no process on the host is running, and
  its own:** it refused only a PID below 1, so a tree registered under a PID no process ever had,
  or under the registering command's own, was handed to the janitor at once and removed at its
  next pass as its owner had ended. The refusal names the PID and writes no row. Stado Desktop's
  Disk screen, its Hosts **Reclaim disk…** sheet, the `registry_cleanup` stage description that
  `stado space report` prints, the space page and the operations storage page no longer say a
  pass under 80% deletes nothing:
  every applied pass takes what nothing uses any more, and the Hosts sheet says which declared
  stages take on the apply it confirms and which only on a later look. Journey
  `tests/space_reclaim/scratch.rs`.

- **A host past the janitor's 80% rule takes the work that fits the room it has:** the agent
  reused the rule (at 80% used the janitor deletes everything the fleet put there) as its claim
  refusal, so a host just past 80% of a large disk, with hundreds of GiB free, refused every
  job and every gate placed on it. The rule still decides what the janitor deletes and is
  published as the note `disk_pressure_active`; admission now compares the volume's free bytes
  with the room work there needs, `diag.disk_room`: the largest scratch a release build on that
  host recorded in the last 7 days (each release worker keeps its newest per product and platform in
  `~/.cache/wisent-compute/release-scratch/`) plus the margin the janitor's latest pass
  reported. A host above it claims at any percentage used; one at or below it refuses with
  `disk_room_short`, its `admission_detail` naming the free bytes and the need with where each
  part was measured. Where the room cannot be judged, the rule decides as before and the
  detail says why. `stado host gates` (blocker, `disk.room`, a `room:` line), Stado Desktop's
  host panel (Room for work), the change queue's host verdict, workload holds, release
  placement (`release_scratch_short` is now scratch plus the published margin against free
  bytes) and `stado host cargo` read the same room. Journey
  `tests/agent_capacity/room_admission.rs`, release gate `room-admission`.

- **`stado space reclaim TARGET --apply` reaches the second look on Linux, and says why and how
  to repair it when a look cannot be recorded:** the reclaim program asked for a modification
  time with `stat -f %m PATH || stat -c %Y PATH`; GNU stat reads `-f` as "the file system", so on
  Linux the look was several lines about the file system followed by the time, and the record's
  first line, read back, never equalled it: every apply on a Linux host kept every second-look
  candidate as unheld on its first look. The program reads the kernel's name once and spells
  `stat` for it, taking one integer (negative before 1970) and nothing else, so on Linux these
  are now taken on the apply after the one that found them unheld and unchanged: stale
  `build_scratch` work, superseded service versions and delivery roots (`delivered_trees`),
  delivered copies and binary backups (`delivery_leftovers`), declared caches
  (`rebuildable_caches`), Cargo build directories (`cargo_build_dirs`), `stado-bootstrap-*` trees,
  and, while the queue store does not answer, `queue_workdirs` job work trees. A time that cannot
  be read, a record that cannot be written or one that reads back otherwise keeps the candidate
  with the cause and the repair: `its modification time cannot be read (…); retained: …`, `its
  age cannot be read (…), so the newest entry is unknown and none is taken: …`, `the second-look
  record <path> cannot be written (…); retained: …`, `the second-look record <path> reads back
  '…', not '…'; retained: remove <path>, then apply again`. Journey
  `tests/space_reclaim/workdirs/second_look.rs`, run on
  darwin-arm64 and linux-amd64 by the release gate `second-look-reclaim`.

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
