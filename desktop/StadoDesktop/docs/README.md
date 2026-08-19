# Stado Desktop

An operator's console over the Stado CLI. Every screen reads through
`stado ... --json` and every write runs the exact `stado` invocation the
screen quotes — the app keeps no state of its own, so anything it shows can
be checked against the same command in a terminal, word for word.

Two screens cover the fleet's services and its builds.

## Services

The screen has three readings of the fleet, grouped in the rail: declared
units, the fleet as the beacons report it, and processes no unit owns. This
page is about the middle group, **Fleet, from beacons**.

### Where the fleet list comes from

The **Managed services** facet is one fleet-wide `stado service list --json`
— the health beacons every host publishes, and nothing else. There is no
ssh and no per-host round trip behind it, so the list stays answerable while
a host is the thing that is broken. For each service whose beacon state is
`failed`, the screen additionally runs `stado service status NAME --json`,
because the list does not carry failure evidence and a red word with no
reason behind it sends the operator to a terminal anyway.

Each row shows the host, the service, the state, the launchd **domain** the
unit's path places it in, when the beacon was reported, and the unit file.

### What the states mean — and what a stale beacon means

`active`, `inactive` and `failed` are what the host's latest beacon
reported. Two other answers are kept deliberately apart:

- `missing` — the beacon exists and does not carry this unit at all.
- `unknown` — the host has published no beacon, or reported no state.

A silent host is not the same fact as a vanished unit. The **Beacon
reported** column prints the beacon's own timestamp verbatim, so an
`active` from a five-day-old beacon reads as five days old: the state is
exactly as fresh as the stamp beside it, never fresher. The console never
re-asks the host to fill the gap — a host that stopped publishing looks
exactly like a host nobody asked, and the stamp is what tells the two
apart.

### When a row is failed

A failed service is named in the alarm above the table, and selecting the
row shows the evidence the CLI's `failure:` block carries: launchd's last
exit status for the label, then where the stderr tail came from — the file,
or the reason there is none (`absent in plist`, an empty file) — then the
tail itself. The panel names `stado service status NAME --json`, the command
that produced it.

### Restarting from here — what it does and does not do

A user-domain unit gets a **Restart…** button. It opens a confirmation
quoting the exact invocation — `stado service restart NAME --host HOST
--json` — and states the two facts that matter: Stado restarts the unit
over the approved channel and reads the host's state before the connection
closes, so the restart is only reported as done if the unit is left
running; and whatever the unit was serving is interrupted until it is back.

That is the whole of it. A restart from this screen is one unit on one host:
no recovery pass, no registry write, no deploy or update, nothing started
anywhere else. Afterwards the list re-reads the beacons, because the
beacon's next word is the one worth reading — a succeeded restart shows as
`active`, a refused one as the state it was refused in.

A **system LaunchDaemon** (a unit file under `/Library/LaunchDaemons`) gets
no button. The console says the privileged bootstrap is required: the unit
loads as root, the approved channel is unprivileged and cannot bootstrap it,
and a button that can only be refused is a lie. It names the two ways
forward — `stado host recover HOST`, or loading the unit as root on the
host itself. One caveat to know: the CLI has since learned one unprivileged
restart for exactly these units — ending the process the approved user owns
so launchd's `KeepAlive` replaces it, when the daemon declares both — and
the console does not offer that path yet. The divergence is known and on
the list; until it closes, the terminal is where a system daemon gets
restarted.

A unit declared in a domain its host cannot have — nobody logged in, no
per-user domain to load it into — is the same lie with a different cause,
so the row says *Restarting it cannot help* instead, and quotes the one
privileged command that installs it as a machine service.

## Builds

One table of build recipes, read from `stado builds list --json` against
the canonical registry — nothing is renamed or derived, so a row here can
be checked against the CLI's output word for word.

### What a recipe is

A recipe is an entry in the top-level `builds` key of the canonical
registry. It names a repository, the branch to watch, one POSIX sh build
command, the artifact paths the checkout leaves behind, and the platforms
it builds for (`darwin-arm64`, `linux-amd64`). A new recipe starts
**disabled**: polling a repository is an explicit opt-in, never a side
effect of writing it down.

Each row shows the source, the enabled and auto-declare flags, and the last
commit the poller saw. Expanding a row shows one run per platform — its
status (`succeeded`, `failed`, `running`), the version it produced, whether
that version was declared to the fleet, and when. A declared platform that
has never built is a row that says so, and a platform the recipe dropped
keeps the run it already recorded: the table shows the last thing that
actually happened, not just the current shape.

### The poller and the per-platform jobs

The control plane checks each enabled recipe at its `interval_seconds`
cadence with `git ls-remote`. A branch head it has not seen enqueues one
ordinary queue job **per platform**, and a worker refuses to claim a job
whose platform is not its own — a Linux build is never built on a Mac
because a Mac was free. Each job clones the branch shallow, runs the build
command in the checkout, and uploads the declared artifacts under the job's
results.

### Where the version comes from

From the tag, never from the poller. After a build finishes, the run's
version is the exact semantic-version tag the built commit carries
(`1.4.2`, `1.4.2-rc1`), a leading `v` stripped, resolved on the build
machine right after the clone — before the build command runs, so the
build cannot change its own answer. An untagged commit produces artifacts
and **no** version; that is the normal case, not a failure.

### Auto-declare — and what a build is not

Auto-declare is off by default and opt-in per recipe. When on, a run that
succeeded *and* has a version declares it as the managed version of the
product the recipe's name selects, on every registry host of the run's
platform, through the same path `stado host declare-version` runs. The
run's `declared` flag turns true only when every matching host took it; an
untagged run declares nothing.

A build is not a release. Builds publish artifacts and record versions;
they never move the fleet's desired state. Promoting a signed release is
`stado release promote` — a deliberate, separate step that verifies the
manifest and its signature — and delivery is still `converge --apply`.

### The kill switch

A top-level `builds_disabled: true` in the registry halts all build polling
fleet-wide without touching any recipe's own flag. It is a registry
setting, not a screen control. What the operator sees here while it is set:
the recipes still list, their recorded runs still stand, no new builds are
enqueued — and a build already running still has its outcome recorded,
though its auto-declare is withheld, because acting on a build is exactly
what the switch takes away.

### Changing things from here

Every write on this screen quotes the exact CLI invocation before it runs.

- **New recipe…** opens the recipe form. The footer shows the exact
  `stado builds add` command line as you type, and every rule the CLI would
  refuse the recipe by is checked in the form itself — kebab-case name,
  `https://` clone URL, relative artifact paths, known platform words —
  with the refusal listed before the button enables. Adding writes once the
  fields hold up; the recipe starts disabled, so nothing is polled until
  enabling says so.
- **Change…** opens the same form on an existing recipe and submits
  `stado builds edit` with only the flags that actually moved — a flag not
  passed is a value the registry keeps. The change is reviewed first, in
  words: a moved source clears the last-seen commit and every recorded run
  (they describe a source the recipe no longer builds, and the red button
  says so), while a moved command, artifact list, platform set or cadence
  keeps both. Enablement is never part of a change.
- The **switch** on a row enables or disables polling, behind a
  confirmation quoting `stado builds enable|disable NAME --json`. Disabling
  stops new builds; a job already enqueued keeps running, and the
  last-seen commit survives, so re-enabling does not rebuild what was
  already built.
- **Run now…** enqueues one build per platform immediately — cadence and
  enable flag notwithstanding — behind a confirmation quoting
  `stado builds run NAME --json`. This is how a recipe is vetted before it
  is enabled.
- **Delete** removes the recipe from the registry behind a confirmation
  quoting `stado builds remove NAME --json`. What already happened stays: a
  job it enqueued keeps running and keeps its results, and a version it
  declared stays the managed version of the hosts that took it — deleting a
  recipe never un-declares anything. To stop building without losing the
  recipe, disable it instead.
