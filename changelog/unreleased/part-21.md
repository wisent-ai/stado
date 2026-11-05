# Changelog Unreleased (part 21)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **A unit launchd runs is no longer started a second time by an old cron fallback:** when every
  launchd domain refused an install made over SSH, the install fell back to a crontab line
  `@reboot /bin/sh ~/.stado/bin/run-<unit>.sh`, and a later install that launchd accepted left
  that line in place. Every boot then started the unit twice; the copy that lost the race for its
  port restarted for ever, and its published state said the unit was down while the other copy
  served. Every install that launchd accepts, in the per-login domains and the system domain,
  now removes the line and its wrapper and says `removed the @reboot cron fallback of <unit>;
  launchd alone starts it`. A crontab it cannot rewrite is refused naming the unit and the wrapper
  the line still names.
- **The network node launchd starts takes its port back from the copy cron started:** a node
  unit loaded before installs removed the cron fallback kept the line, so at every boot cron's
  copy took the node's port and launchd's node failed to bind and restarted every ten seconds
  (3130 times on one Mac). The node its unit started now reads who holds its port; a listener
  whose parent ran this unit's cron wrapper is ended (SIGTERM, then SIGKILL after five seconds),
  the cron line and wrapper are removed, and the bind is tried again, saying `port <p> was held
  by pid <pid>, a copy of this node cron started from <wrapper>`. Any other holder is left alone
  and the bind refusal stands. `tests/network/cron_copy.mjs` runs it on a dedicated account.
- **A start or restart waits for launchd's next spawn of a job it scheduled:** macOS refuses the
  first spawn of a program whose code identity an install just changed (`OS_REASON_CODESIGNING`)
  and runs the next one; the postcondition read the unit between the two, answered `loaded with
  no pid`, and `stado product install stado` rolled back a release that was about to run. A job
  in `spawn scheduled` is now judged after launchd's next attempt, and a unit still without a pid
  is reported with launchd's state, run count and last exit reason.
- **A slow start is no longer refused as a hung one:** a start step past its longest was judged
  stuck even when nothing below it was overdue, and `stado serve` judged its own start, so a
  resolver that took six seconds longer than ever before ended the serve. A start is now judged
  stuck only when a wait below its step is past its own longest, and never by the serve itself.
- **An npm product installs from source again:** the recipe installed the checkout directory, whose
  `prepare` script (`npm run build`) npm ran in a copy without the package's own development
  dependencies, so Weles failed with `tsc: command not found` and no Weles from source could be
  installed. The committed tree is exported, `npm ci` installs its lockfile and runs its build, and
  the packed result is what the product's versioned prefix installs.
- **Host GUI automation runs on a busy host:** `stado workload run gui-automation` sets up the
  host's own Accessibility grants and Apple two-factor helper, yet it took a capacity hold, so a
  host whose load left no free core refused it with `has no room for gui-automation`. It is
  declared a maintenance kind and takes no hold.
- **A fleet whose object API stood on a host that is gone can be brought back without it:**
  `stado placement move --authority` took the registry authority on a host from a silent one,
  but the registry was still read from the fleet object API placed on the silent host, and
  `stado placement role move` fences and copies that host's store, so it needed the host it
  was leaving to answer. Every registry read on the new authority then stood on the machine that
  was gone. `stado placement role take SERVICE --reason WHY [--check] [--json]`, run on the
  host that holds the registry authority, makes its own object API the route's active endpoint
  and writes the registry from its own last-known-good copy into it, handing the same registry
  to every other host that answers. It is refused while the active host's store answers a
  registry read within the resolver's refresh interval (`move the role with its store instead`),
  on a host whose copy names another authority (`take the authority first: stado placement move
  --authority --to-host HOST`), on a host that keeps no copy, and when this host's own object
  API does not answer a registry read. The receipt, kept in `stado placement history`, names
  what was not recovered: whatever the silent host's store held and this host's does not.
- **A request only a statement can answer is answered in the same call:** `stado capabilities
  call FAMILY --consumer C --await-statement` waits, when the attestation alternative is reached
  and no statement answers the request yet, for `stado capabilities attestation record` to write
  one, and prints it. The consumer's attestation directory is watched by the kernel (kqueue,
  inotify), armed before each read, so a statement written between the read and the wait still
  wakes it; the wait is a `sesja` wait, listed with every other. A Weles run that a person
  attends (`WELES_AWAIT_STATEMENT=1`) is answered page question by page question in one session
  instead of refused at the first and started again, which repeated every sign-in before it.
  The Compute providers screen's call operation offers the same wait.
- **A route taken from a silent host answers at once, not after the dead connection ends:**
  every reader of the object API refused with "this host's resolver holds 1 connection(s) on
  that adapter unanswered longer than the 300s refresh interval" after `stado-object-api` was
  taken on another host, because the connection still hanging on the switched-off host was
  counted against the route that had moved away from it. A held wait now counts only while the
  service's route, in this host's registry copy, still names the host it waits on; the
  resolver opens the next connection to the new active host, and the old one says nothing
  about it. The port the taking host serves on is also read from the registry the command
  acts on, not through the silent store, so `role take` no longer stands on the host it takes
  from.
- **A second copy of a service is found by what it runs, not by what it is called:** a host
  carried a second `stado serve` under `com.wisent.always-on.stado-object-api` beside
  `com.wisent.stado`, and the label checks never saw it, because they matched names the fleet
  had retired (`com.wisent.compute.`) and each new spelling slipped past them. The fleet-shape
  sweep (`stado doctor`, and the coordinator's tick) now groups every loaded unit this fleet
  wrote by the program it declares — the binary without its directory, and its first
  subcommand — and reports `one-unit-per-service` for each program more than one unit runs,
  naming every label and the one the registry declares, with `stado service bootout` for the
  others. Units started on a schedule are jobs, not copies, and are not compared. It measures
  how many units it compared, so a quiet sweep says what it looked at.
