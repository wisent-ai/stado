# Changelog Unreleased (part 11)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **`stado host cargo` compiles the revision it was given:** the delivered export carries its
  commit's time on every file and the host's Cargo target is shared by every run, so a revision
  committed before that target's last compile kept the earlier revision's build of each workspace
  crate, and a check, clippy or test of it judged code it was never given. Every delivered file now
  takes the latest time `touch -t` states, so no compile — not even one of another revision that
  held the target's lock while this run waited — is newer than its sources; dependencies from
  registries stay compiled.
  A Cargo run that exits 0 without Cargo naming the manifest's own package as compiled or checked
  is refused instead of passed: `<host>: cargo <operation> of <commit> exited 0 without compiling
  or checking <package>, so its verdict is another run's output; run it again`.
- **A product operation turned away for the disk janitor's turn says retrying helps:** without
  `--wait`, `stado product install` (and every building product operation) found the janitor
  asking running work for its turn and refused as a rule, `retrying will not help`, exit 1, though
  the turn ends once the pass has run. It is now a retryable wait: exit 69, `retry later`, and the
  sentence says to run it again once that pass has run or with `--wait`.
- **A published release installs itself on the hosts that run its product:** a delivery whose
  target selects a product went only to the hosts someone listed with `stado release destinations
  set`, so a product nobody listed (Skarbiec among them) was published and reached no host. With no
  declaration, its destinations are now every registry target whose `managed_versions` names the
  product; a declaration still places a product on a host that does not run it yet, and
  `destinations show` reports `running` or `declared`. A product no host runs and no declaration
  places is refused naming both ways.
- **An object body that stops arriving after its headers is named, not waited on forever, and the
  store reads a range alone:** the object route's GET read its body outside the wait mechanism,
  so a 1.6 GB input whose headers came and whose bytes did not held a check for over half an hour
  at no CPU. Every body of the object route (object, release and metadata answers) is now read as
  a wait, said on stderr with the byte it starts from, and ends as stalled when no byte arrives
  for the refresh interval of this host's resolver, the interval the route's sends are already
  abandoned at: `<operation> of <uri>: no byte arrived for <n>s, the refresh interval <who
  declares it>, after <bytes> bytes of this body`. An object is read one chunk-sized range at a
  time from its first request, and a range that stops after some of its bytes arrived is asked
  again from the first byte it lacks; a range that brings no byte ends the read with its cause.
  The store reads each range of a stored object alone instead of the whole object per range.
