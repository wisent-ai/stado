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
