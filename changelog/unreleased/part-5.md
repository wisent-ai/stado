# Changelog Unreleased (part 5)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- `stado_database::sync::Client` no longer panics with Tokio's `Cannot drop a runtime in a
  context where blocking is not allowed` when it is dropped, or refused while connecting, inside
  a product's own async runtime. A product that held it there answered an unreachable database
  with that panic and exit 101 instead of the refusal naming the step and Stado's answer, unless
  it opened and released the client on threads of its own. The client
  now releases its runtime by leaving a multi-threaded worker for the wait, shutting it down
  without waiting on a current-thread runtime, and dropping it anywhere else.
- A child's wait is measured under its program and the first argument that is neither an option
  nor an assignment again: an argument naming a path that exists no longer moves the subcommand,
  so `cargo build` beside a `build/` directory or `git status` beside a file named `status` is
  measured as its own operation, and no spawn stats its arguments. A caller whose child takes an
  option's value before its subcommand (`git -C <checkout> status`) names the operation with the
  new `stado_wait::output_as` or `status_as`.
- The delivery routine runs again on the host that declares it. Its pass was fired only from the
  coordinator's tick, which a host serving with `--release-interval-seconds` but no
  `--coordinator` never runs, so a plan declared there ran one pass and none after. Every serve
  that coordinates or reconciles releases now fires a pass once the plan's cadence has passed
  since the last pass was attempted (a pass the operator's rulings refused included), never
  while a pass holds the lock, and says once why it fires none.
  `stado ci plan set` takes the cadence as `--every SECONDS` (required); a plan declared without
  one fires nothing until declared again, and `stado ci plan show` prints the cadence and when
  the next pass is due.
- `stado product install` attributes the refusals it used to report as `unknown; retrying will
  not help`: a source commit neither main carries is `refused` and says how to make it
  installable; a repository with no canonical checkout, and a `--catalog` file its checkout has
  not committed, are `config` (the latter refused before anything is read, naming the file, the
  checkout and the commit, where it used to surface as git's `exists on disk, but not in`); and a
  signing credential read whose `stado credentials get` exited with the retry code because
  Skarbiec did not answer is `infra_down`, exit 69.
- `stado serve` whose product sync installed the release that replaces it exits with the retry
  code, 69, which launchd and systemd both restart into the installed release, and no longer
  keeps that delivery in `~/.stado/serve-stops.jsonl` as a stop: every successful delivery used
  to read as a crash (exit 1) and could later count as the build stopping again.
- A release rebuild after a failed platform attempt goes to another builder of the platform
  first; the host the failed attempt ran on is chosen only when no other builder can take it.
- An installation whose operator named a new signing identity says
  `<path>: signing identity <previous> -> <new> (the operator's named choice)` for each placed
  file, beside the receipt's `identity_changes`.

### Added

- `stado product signing report` without a product reads every installation recorded on the
  machine, each row naming its product and surface, and exits 1 when any native file is not
  stably signed (ad-hoc, unsigned), so one read names every installed program that asks the
  privacy questions again on each build.
