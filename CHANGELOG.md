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
- **The checkouts fleet sessions clone no longer fill a host's home volume, and the janitor takes
  back only those a session is proven to have cloned:** an `omp-session` cloned its repository
  into `~/Documents/CodingProjects/Wisent/<repository>` on the host's home volume, whose 80% rule
  stops the host from claiming work, and no cleaner owned those trees. It now clones into
  `<work root>/session-checkouts/<repository>` and writes the clone marker
  `.git/stado-session-clone` naming its job; its `CARGO_TARGET_DIR` and `TMPDIR` are in the job's
  work tree, its log names where it works, a host that declares no work root is refused naming
  `stado space root <host> --path <directory>`, and an unwritable session checkouts directory
  fails the job naming the same verb. The janitor's `session_checkouts` cleaner takes a checkout
  (under the home on every pass, under the work root at the rule) only on that proof, or, for a
  checkout an older release cloned under
  `~/Documents/CodingProjects` on a host that is not shared, on this host's queue record of the
  `omp-session` whose command names exactly that checkout and during which its `.git` was made;
  every other checkout, however clean and pushed, is kept as `not_session_checkout`. A proven
  checkout is kept while it holds an uncommitted change, no remote, a commit any ref reaches and
  no remote has (a local tag included), a stash or another worktree (`unpushed_work`, named; git
  is asked with `status.showUntrackedFiles` and submodule ignore settings overridden, so a
  checkout's own configuration cannot hide an untracked file or a changed submodule), while
  git cannot answer, a process works in it or a service or unit names it; otherwise it is moved
  aside and removed. `lsof` or `ps` that cannot be read is now named among the errors of every
  cleaner that reads them. Journeys `tests/space_reclaim/workdirs/session_checkouts.rs`,
  `tests/space_reclaim/workdirs/session_keeps.rs` and `tests/workload/omp_session_run.rs` (the
  refusal without a work root and the real worker, on darwin-arm64 and linux-amd64).
- **A host whose work root is a roomy data disk is never full because its system disk is:** the
  disk-full rule judged the fuller of the home's and the work root's volumes, and every process the
  agent did not start (`stado space reclaim --local`, `stado space candidates`, the health beacon)
  judged the home alone, so a host with a small system disk and terabytes free on its work root
  ran cleanup passes that refused claims and was reported full. The rule, admission, `stado host
  gates`, `stado space report` and the beacon's `disk_avail_gb` now judge the volume the fleet
  writes to: the declared work root's, read from the agent's declaration, `STADO_WORK_ROOT`, or
  this host's entry in its last-known-good registry copy, else the home's. The home's own volume
  is listed among the others; while it is at the threshold the fleet's rebuildable caches under it
  are taken without refusing claims, and the proven session checkouts under the home are taken on
  every pass since nothing reuses them; the work root's wait for the rule.
- **Every fleet session works on the work root, and every job's package downloads land there:**
  `jeden-session` (detached, and attached to another host) works in the host's checkout under
  `<work root>/session-checkouts/<repository>`, cloned from this machine's origin when absent with
  the clone marker written, with its Jeden ledger under `<work root>/jeden-sessions` and its Cargo
  target and temporary directories in the job's own tree; the home workspace works in its job
  tree. A host that declares no work root is refused naming `stado space root <host> --path
  <directory>`. On a host with a work root every job's pip, uv and npm caches are
  `<work root>/package-caches/<manager>`, taken by the `package_caches` cleaner.
- **The stado library compiles again after the fleet database re-address:** `stado database tls
  reissue` and `stado database migrate`'s reuse of a pending fleet server read the item's port
  through a match whose refusing arm named the conversion error in one alternative of an
  or-pattern and not in the other (`E0408`), so no revision since then built on any platform. The
  arm binds nothing, and an item whose port is missing or wider than a TCP port is still refused
  as naming no TCP port.
