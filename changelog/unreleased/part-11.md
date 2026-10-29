# Changelog Unreleased (part 11)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **`stado change`, the submit queue, one per repository:** `stado change submit CHECKOUT
  --commit SHA [--defect ID]...` queues a commit an agent made on any `main`; `stado change run
  CHECKOUT [--host NAME]... [--integrated DIR]` takes the repository's queued candidates oldest
  first, one at a time, writes each candidate's own commits again on the remote `main` (no working
  tree or index of the checkout is touched), runs the product's declared gates on the result —
  the locks, module declarations and `format` gates here, every `lint` and `test` gate of every
  declared platform on that platform's hosts at once (a Cargo gate as `stado host cargo` runs it,
  any other exactly as the manifest declares its argv, in the delivered tree), a `cargo test`
  gate that selects no target cut into one shard per test target, each compiling action asked of
  Oko as the work of the agent session that submitted the candidate (`oko admit --session`) —
  answers a shard that already passed on the same tree from its action cache, classifies the
  change's risk from the product's catalog `submit` declaration (criticality, review paths,
  changed lines, whether a compiling gate checks it), has a high-risk change reviewed by Tama's
  harsh review before it lands and a low-risk one after, queues the reviewer's repairs once as a
  follow-up candidate (a follow-up's own repairs are named for its author, not queued again),
  pushes to `main` as a fast-forward (a `main` that moved takes the candidate back to the rebase),
  and records each defect repaired in Oko and, with `--integrated`, as one backlog line per
  defect; a register write that is refused is kept on the candidate. A candidate the pass cannot
  judge (the remote, the fleet or the harsh review does not answer) stays queued and the pass
  exits non-zero with the cause; one whose gates Oko's admission does not admit (a ruling, a
  closed defect) or does not answer for stays queued while the candidates behind it run, and the
  pass then exits non-zero naming it; a later pass also takes a candidate a stopped pass left
  running. A change of a product whose catalog record declares no `submit` is refused, naming
  the declaration to add, and so is a high-risk candidate that names no `--defect`, since the
  harsh review is scoped to one. A host that cannot run its shard is left out of the pass and the
  shard given to the platform's other hosts. `stado change status|list|cancel` read and withdraw
  candidates. Stado Desktop's Queue screen carries the same five operations. The catalog gains
  the optional per-product `submit` declaration (`criticality: critical|standard`,
  `low_risk_max_changed_lines`, `review_paths`); Stado and Oko declare themselves `critical`,
  Trends `standard` (low risk up to 40 changed lines outside its manifests and workflows).
- **The submit queue refuses a change too big to review whole:** the catalog `submit` declaration
  gains `max_changed_lines`, the most lines (added plus removed) one change of the product may
  change. `stado change submit` refuses a commit whose own commits change more past their merge
  base with `main`: `<commit> changes <n> lines in <f> files past its merge base <base> with main,
  more than the <max> one change of <product> may (its catalog submit.max_changed_lines); split it
  into changes of at most <max> lines and submit each`, and a product whose declaration states no
  number. Stado and Oko declare 2000, Trends 400.

### Fixed

- **A reviewed candidate no longer holds the head of the submit queue:** a harsh review before
  landing whose reviewer staged repairs, rewrote only the message, or ended with no round covering
  what it left kept the candidate queued at the head, so every `stado change run` ran the same
  review again first (an hour for a 50-commit chain) and no candidate behind it moved. Such a
  candidate is now `repaired`: its record names the reviewer's commit and tree (`repairs`), and
  that commit (the reviewed one when nothing was staged) is queued at the tail as its follow-up,
  carrying the same defects; a message-only repair is the reviewer's message over the reviewed
  tree, whose gates the action cache answers. A follow-up whose own review ends that way is
  refused, naming the commit for its author. `stado change list --state repaired` lists them.
- **The harsh review judges a candidate as written by the session that submitted it:** Tama took
  the author of a tree no harsh-review queue entry names to be the session invoking the review,
  which was the session running the pass, so a candidate the queue reviewed for another session
  was judged as that runner's own. Before the review the candidate is entered in Oko's harsh-review
  queue (`oko review harsh enqueue`) as staged by its submitting session (the operator when none
  did). Oko keeps one open entry per author, so when the author's earlier entry still waits for
  its round the review runs anyway and its record begins with why the author was not entered.
- **A high-risk change that names no defect is refused at submit:** `stado change submit` took
  `--defect` as optional, and `stado change run` refused such a candidate as `names no defect`
  only after all its gates had run. A candidate the risk rules already call high over its own
  commits and declared gates (a critical product, a review path, more lines than
  `low_risk_max_changed_lines`, no compiling gate) and that names no defect is now refused at
  submit: `<commit> is high risk (<reasons>), so it is reviewed before it lands, and that review is
  scoped to the defect it repairs; submit it with --defect ID`.
- **A declared gate the submit queue runs on a fleet host finds Stado:** `stado change run` ran a
  gate whose argv calls Stado (Trends' `stado release gate app check …`) in host cargo's prelude,
  whose `PATH` holds no `~/.stado/bin`, so it exited 127 `stado: command not found` on every host
  and refused the candidate for the host's environment. A declared gate now runs with the host's
  installed Stado as `STADO_BIN` and its directory first on `PATH`, as a release worker's job runs
  the worker's own Stado.
  A declared gate also reads the history records a release build's archive carries beside the
  commit's files (`.wisent-provenance/baseline.json` and the published diff), so an application's
  version gate no longer refuses every candidate with `the source archive carries no verified
  baseline provenance`.
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
