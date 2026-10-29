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
  harness configuration and no provider credential reaches the host. `--context FILE`, which can
  be repeated, gives the session its starting instructions. The files are read here in order,
  published to the job store as the harness's own `AGENTS.md`, and declared as the job's input,
  so the host's agent writes them into the session's agent directory before it starts.
- **`stado workload harness status|install KIND --target HOST`:** the harness a session kind
  declares (`harness` in `data/work/workloads.json`). `install` carries this machine's copy to the
  host's managed path through a managed run, checks it there with its own `--version`, and
  replaces the old copy with one rename on the host's volume. A host whose declared
  `release_platform` is not this machine's is refused before anything is copied: `<HOST> declares
  release platform "<P>" and this machine's <program> is built for <Q>; install the harness from
  a <P> machine of the fleet`.
- **`stado workload results JOB --into DIR`:** reads the result lines a detached session wrote on
  its host. It appends to `DIR/<name>.jsonl` every line that file does not already hold, so
  repeated runs while the session works add only what is new.
- **`stado quality hooks install|status|remove --product P|--all [--json]`:** the pre-push gate
  of a product's checkouts on this machine. Every repository the catalog names for the product
  (owner, surfaces, installations) has its canonical checkout found, and `install` writes Stado's
  Git `pre-push` hook there, which hands every push to `stado quality check --pre-push
  --if-declared` through the installed Stado. Running it again changes nothing; a hook another
  Stado wrote is repaired; a hook that is not Stado's (a Git LFS upload) is kept as
  `pre-push.chained` and runs after the gate with the same ref updates. A checkout whose
  `core.hooksPath` sends Git to another directory is left and named, since that directory's
  `pre-push` decides what a push runs. `status` reads each checkout's state (`installed`,
  `outdated`, `absent`, `foreign`, `governed`, `no-checkout`, or `unreadable`, which exits
  non-zero) and `remove` takes the hook out and puts back the chained one. Stado Desktop's
  Releases operations run all three.

### Fixed

- **`stado credentials token mint` refuses a retired agent identity:** it minted a grant for a
  consumer such as `stado-local-agent`, and only the host's own configuration refused it later,
  after the bearer had been minted and delivered. The mint now refuses with `credentials token mint
  refuses consumer <name>: "<name>" is a retired agent identity that no host's configuration
  accepts; use "stado" locally or a new scoped *-agent consumer`, which is the same rule the
  configuration check applies.

- **A push to `main` runs the pushed commit's `lint` gates, not only its `format` gates:** a commit
  pushed straight to `main` skipped the clippy gate the submit queue and every installation run,
  so `main` went red for every session building from it. `stado quality check --pre-push` now
  runs the `lint` gates a commit pushed to `main` declares on fleet hosts exactly as `stado change
  run` runs them (`stado host cargo` for a Cargo gate, the action cache answering a tree that
  already passed, Oko asked to admit the compile as the pushing session's work) and refuses the
  push with each refused gate's own report. The queue records the commit it is about to push as
  the candidate's `landing`, and the pre-push check lets that exact commit through without
  running its gates again. The hosts come from their capacity publications; `--host HOST`
  names them by hand, and a push no host can take is refused naming the queue, whose pass
  names its hosts. A Stado that cannot read the queue's records (a newer queue wrote a state it
  does not know) says so and runs the commit's gates, instead of refusing every push.

- **A resident unit that starts Stado with an option it refuses is rendered again by that Stado:**
  a unit written by an older release, or edited by hand, made the installed executable exit on
  its argument vector at every start, and init restarted it without end. When the refused
  argument vector is exactly the resident unit file's, the refusing executable now starts its own
  `stado service ensure stado --host <this host>` before it exits, says so on stderr and writes
  the converge's answer to `~/.stado/logs/resident-converge.log`.

### Removed

- `stado host run build`: `stado host cargo … build` replaces it. The old verb compiled with the
  host's default rustup toolchain, because rustup reads the toolchain from the directory it starts
  in and never from `--manifest-path`. It also left the export, delivery and cleanup to the caller.
