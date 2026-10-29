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
  and the exit status is Cargo's. The build is given `WISENT_SOURCE_COMMIT` and
  `WISENT_SOURCE_SHA256`, as a release worker gives them, so a build script that stamps its
  revision does so without a Git directory. The receipt holds the commit, the archive digest,
  Cargo's streams, the binaries with their SHA-256, and the free space the run took.
- **Workload kind `omp-session`:** `stado workload start omp-session --workspace REPO --task TEXT
  --max-time DURATION [--target HOST] [--model M] [--allow-write] [--allow-command]` places one OMP
  coding-agent session on the fleet host with the most room, as a detached queue job. The host
  works in its own checkout of REPO at `~/Documents/CodingProjects/Wisent/REPO`. That checkout is
  cloned from this machine's origin when absent and fetched when present. The session pushes with
  the fleet's GitHub credential, which the host's agent hands over as a job secret and git reads
  through an environment-declared credential helper. It writes result lines into
  `$STADO_SESSION_RESULTS/<name>.jsonl` below the job's output. `stado job watch` follows it and
  `stado cancel` stops it. `--max-steps` stays `jeden-session`'s bound. The harness's model
  requests go to the gateway the declaration names (`harness.gateway`): the host's own Stado
  resolves its loopback route for the declared consumer, and each declared provider is routed
  there with the gateway bearer the job is handed in `harness.secrets`. The harness's agent
  directory lives in the job's work tree, so nothing is written into the host account's own
  harness configuration and no provider credential reaches the host.
- **`stado workload harness status|install KIND --target HOST`:** the harness a session kind
  declares (`harness` in `data/work/workloads.json`). `install` carries this machine's copy to the
  host's managed path through a managed run, checks it there with its own `--version`, and
  replaces the old copy with one rename on the host's volume.
- **`stado workload results JOB --into DIR`:** reads the result lines a detached session wrote on
  its host. It appends to `DIR/<name>.jsonl` every line that file does not already hold, so
  repeated runs while the session works add only what is new.

### Removed

- `stado host run build`: `stado host cargo … build` replaces it. The old verb compiled with the
  host's default rustup toolchain, because rustup reads the toolchain from the directory it starts
  in and never from `--manifest-path`. It also left the export, delivery and cleanup to the caller.
