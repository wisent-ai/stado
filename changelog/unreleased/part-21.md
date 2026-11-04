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
