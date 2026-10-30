# Changelog Unreleased (part 15)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **Adding a machine to the fleet declares every way it fails:** `stado fleet enroll`, `stado
  fleet join` and `stado fleet approve` raise only declared causes (`fleet.enroll` 57, `fleet.join`
  10, `fleet.approve` 63), each naming what failed, why, the state observed and what fixes it, with
  its code and exit status: the registry read and write by the class the store answered, the
  enrollment catalog's gates with the methods the fleet does allow, the channel key, the
  `--install-key` session, each way OpenSSH ends the probe (unresolvable, refused, timed out, no
  route, key refused, host key changed), the platform, the install and its rollback, and the
  network join. A failure prints `fix: ...` under its `Error:` line and names its cause; `--json`
  answers it as JSON with `cause` and `remedy`.
- **`stado errors list|show|check`:** `list` names the operations that declare a failure catalogue,
  `show OPERATION` prints each cause with its fix, code, exit status and test (`--json`,
  `--markdown` for the documentation page), and `check` is the new `error-standard` quality gate:
  it refuses a catalogue that is not one and an error message holding a generic sentence ("did not
  succeed", "operation failed") beyond `stado-rs/errors-baseline.json`, which may only shrink.
- **The submit queue runs by itself:** the Stado CLI installation declares a host routine
  (`stado schedule create --host-store`, every ten minutes, overlap skipped) running `stado change
  run` with no checkout, which carries every repository of this machine's queue that has a
  candidate waiting, each from the checkout its candidate was submitted from. Nobody ran the pass
  before: it ran inside agent sessions, whose end killed it, its reviewer and the candidate it
  carried. `stado schedule create --host-store` declares any routine into this host's own store.
- **Each candidate without a verdict says what it waits on:** `stado change list` and `status`
  print `waits on: <the next pass | a pass | the remote | Oko's admission of its gates | a host for
  its gates | a harsh review round>` with the command that unblocks it, and `--json` carries it as
  `waits`; the record keeps the step as `waits_on`.
- **`stado change review ID --findings FILE --improvements FILE`:** records a review round over a
  queued or running candidate beside its record (`<id>.rounds/`), by the agent session the command
  runs in as its harness names it (`tama sessions self`, else the transcript `PI_SESSION_FILE`
  names), never a name typed; the submitting session and a command no session runs are refused. The
  round is over the commit the next pass lands (the candidate as the last pass wrote it on `main`)
  and keeps its tree: a round over a tree a moved `main` replaced decides nothing. At the review
  before landing each reviewer's newest round over the tree counts, so one reviewer never clears
  another's findings: any finding refuses the candidate naming it, and rounds that all found nothing
  admit it. Tama's harsh review is one adapter, run only when no round is recorded; without Tama
  the candidate waits on `a review round over commit <sha> by a session that is not its author`,
  naming this command.
- **A host whose worker cannot reach the fleet store says why in `stado host gates`:** every
  capacity write, every agent start and every tick that met a store that did not answer leaves its
  outcome on the host in `~/.stado/logs/capacity-publication.json` (step, time, the store's words,
  since when it refuses, the Stado version). `host gates` reads it over the host channel, which needs
  no store, and prints `its worker cannot reach the fleet store: its <step> at <time> was refused,
  refused since <time> (Stado <version>): <error>` where it said only `nothing published`; a host
  with no record is named as one whose worker never ran under a Stado that keeps one. `--json`
  carries it as `publication_attempt`.
- **`stado space candidates [--min-bytes N] [--json]`** ranks what holds this machine's disk as
  the next cleanup candidates: agent output first (tagged build trees, Cargo build directories, job
  work trees, harness logs, a checkout's `.build`/`.build-*`/`target`), then caches (compiler,
  model, package, Xcode), then the owner's own large files and folders (a game library among them)
  as proposals only. Each row has its bytes, producer, safety (`rebuildable`, `refetchable`,
  `scratch`, `user_owned`), whether the janitor's pass takes it (`automatic`) or only proposes it,
  and the cumulative bytes; the answer says how much must go to be under the 80% rule and lists
  the janitor's newest removals. Desktop shows it in Settings, "Cleanup candidates on the API host".
- **The janitor's pass at the 80% rule takes Cargo's build directories itself
  (`cargo_build_dirs`):** every `$CARGO_HOME/build/<xx>/<hash>`, tagged or not, whole, unless a
  build holds a `.cargo-lock` in it (`held_by_build`) or Stado runs from it; before, only the fleet
  stage `space reclaim --stage cargo_build_dirs` did, and a laptop at the rule kept tens of GiB.
- **The janitor records what it removed** in `~/.cache/wisent-compute/reclaimed.jsonl`: one line
  per tree (path, bytes, freed bytes) and one per other cleaner per pass (items, bytes). A damaged
  line is refused by `space candidates` naming the line and the fix.
- **`stado space scratch register|list|forget`: the scratch register.** A job, an agent or a verb
  registers a tree it made (an export under a checkout's `.build/`, say) with its creator, purpose,
  owner session and owner process; at the 80% rule the janitor's new `registered_scratch` cleaner
  removes every registered tree whose owner process has ended, least recently changed first, keeps
  the rest (`owner_session_running`), records each removal in `reclaimed.jsonl` and forgets the
  row. A tree that was never registered is never taken. `space candidates` ranks registered trees
  as automatic agent output. A path outside the home, inside `~/.stado`, the janitor's state or
  `~/.ssh`, a link, a path through a link or with `.`/`..`, an absent tree, a registration
  without `--owner-pid` and a damaged register are refused naming them and the fix; the register
  keeps the tree's real path, and a tree that no longer resolves to it is kept, not removed.
- **The janitor takes Xcode's per-project build directories at the 80% rule
  (`xcode_derived_data`):** each `~/Library/Developer/Xcode/DerivedData/<project>-<hash>` whole,
  unless a running process has a file in it open (`lsof`) or names it (`held_by_process`); when
  either table cannot be read it takes none (`process_table_unread`). `ModuleCache.noindex`,
  archives and device support are never touched. Each removal is in `reclaimed.jsonl`, and `space
  candidates` ranks each project directory as automatic agent output.
- **The janitor's second tier: package managers' download caches (`package_caches`).** When the
  agent output a pass took leaves the volume at the 80% rule, or without room under it for the
  next build, the pass takes Cargo's git `checkouts/` (never `db/` or the registry), npm, Bun,
  SwiftPM, pip, uv, Homebrew and VS Code's update staging, largest first, and stops once the
  volume has that room (`room_for_the_next_build`). A cache a running process holds (a file
  open, a working directory, its path in arguments; Cargo's package-cache lock for its
  checkouts) stays (`held_by_process`), and Cargo's checkouts stay while any `cargo` or `rustc`
  runs (`build_running`). A cache is moved aside in one rename right after its holders are read,
  so a tool starting during the removal makes a new one. Each removal is in `reclaimed.jsonl`;
  `space candidates` ranks them as automatic caches. Products' own application caches and the
  owner's files stay proposals.
- **A pass leaves room under the rule for the next build (`margin`).** A pass that stopped just
  under 80% left the host under a GiB, and the first build put it back over and closed its gates.
  The room is measured, not declared: the largest Cargo build directory the pass measured under
  Cargo's home, or that `reclaimed.jsonl` says a pass took in the last 7 days. A host under the
  rule but short of that room runs a pass too, which takes only the package managers' caches:
  agent output (an idle agent's build directory and its incremental state) goes only at the rule.
  Each report carries `margin` (bytes, the directory it was measured on, and where that
  measurement came from).
- **A crash-looping declared unit pages the operator.** The first health pass that reads a unit
  failing (no process, a nonzero last exit, the supervisor's run count) sends one page through the
  registry's `operator_contact` channels with the unit, the verdict and the last line it wrote to
  stderr, and keeps what the channels answered in `~/.stado/failing-units.json`. A page no channel
  took is tried again on the next pass. The unit's beacon `detail` says `operator page: …; Oko
  defect: …`.

### Changed

- **A running `cargo` build keeps its build directory.** The janitor's guard locked only
  `.cargo-lock`, but a Cargo build-dir laid out apart from the artifacts is held through
  `.cargo-build-lock` (and a target through `.cargo-artifact-lock`), so a pass took directories
  running builds were compiling into. The guard now locks all three and keeps a directory any of
  them is held in (`held_by_build`).
- **The Xcode and package-cache cleaners read what holds a tree right before taking it,** under
  the cleanup lock, so a build started after the pass began is seen; a directory entry that cannot
  be read is reported, not skipped silently.
- **The crash-loop defect hand-off to Oko states how a caller meets it (`--blocks-nothing`),** which
  Oko had refused on every pass since it began requiring it. A host without Oko says `Oko not
  installed` once per loop instead of failing every pass, and the operator's page goes out either
  way.
- **Enrollment refuses a request's form before reading anything:** a name that is not a lowercase
  target identifier, an `--ssh` destination ssh would read as an option, and a `--kind` nobody
  declares are refused with exit 2; `approve` refuses a fleet nobody declared before writing.
- **A stored join request that is not JSON is `config`, exit 1,** not a retryable outage (exit 69):
  retrying never repairs it; `stado fleet reject` and a new `join` do.
- **A candidate left running by a pass that is gone is resumed and named:** a pass records itself
  on the candidate it carries; a running candidate whose repository's pass lock is free (the lock
  the kernel releases when the pass's process ends, killed or not) is listed as `waits on: a pass;
  the pass that carried it (pid N) is gone …`, and the next pass resumes it.
- **The resident pass asks Oko as the fleet's daily build:** a pass under no agent session asks
  `oko admit compile|test` with no session first, the run the operator's `@fleet-budget` consents
  admit, then as the submitter's work; its refusal names the subject that would admit it
  (`<action>:<product>@fleet-budget`). A pass inside a session still asks as the submitter only and
  says so.
- **A fleet store that does not answer no longer stops the queue's gates:** when the hosts'
  capacity publications cannot be read, this host runs the gates of its own platform and the
  registry's hosts of the other platforms run theirs unjudged, each said with the store's error, so
  a repair of the fleet store can land through the queue.
- **A candidate stacked on a waiting candidate is measured over it:** `stado change submit`
  measured a commit's size from its merge base with `main`, so a change stacked on a queued one was
  refused for the lines below it, which that candidate had already been measured for. A commit
  that holds a queued or running candidate's commit `main` does not is now measured past the
  nearest such candidate, and the refusal names it: `<commit> changes <n> lines in <m> files past
  candidate <id> it is stacked on (<its commit>), more than …`.
