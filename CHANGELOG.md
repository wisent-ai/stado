# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.69 – 0.23.70](changelog/0.23.69-0.23.70.md)
- [0.23.68 (part 1 of 10)](changelog/0.23.68-part-1.md)
- [0.23.68 (part 2 of 10)](changelog/0.23.68-part-2.md)
- [0.23.68 (part 3 of 10)](changelog/0.23.68-part-3.md)
- [0.23.68 (part 4 of 10)](changelog/0.23.68-part-4.md)
- [0.23.68 (part 5 of 10)](changelog/0.23.68-part-5.md)
- [0.23.68 (part 6 of 10)](changelog/0.23.68-part-6.md)
- [0.23.68 (part 7 of 10)](changelog/0.23.68-part-7.md)
- [0.23.68 (part 8 of 10)](changelog/0.23.68-part-8.md)
- [0.23.68 (part 9 of 10)](changelog/0.23.68-part-9.md)
- [0.23.68 (part 10 of 10)](changelog/0.23.68-part-10.md)
- [0.23.67](changelog/0.23.67.md)
- [0.23.66 (part 1 of 2)](changelog/0.23.66-part-1.md)
- [0.23.66 (part 2 of 2)](changelog/0.23.66-part-2.md)
- [0.23.42 – 0.23.65](changelog/0.23.42-0.23.65.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

### Fixed

- **The App Store Connect bearer lifetime is read from Apple's stated limit, kept with its
  source:** `stado credentials signing profile ensure --provider apple` signed its bearer for a
  lifetime written into the code as 20 × 60 seconds. The lifetime now comes from the provider
  declaration `provider/app-store-connect.json`, which names the Apple page it was read from, the
  day it was read and the rule it states; a declaration without a whole number of seconds is
  refused by name before the bearer is signed.

- **The record of a worker's publication attempts follows the worker's own attempts, not a
  minute of its own:** `~/.stado/logs/capacity-publication.json` skipped writing an attempt whose
  outcome, step and error matched the record unless the record was a minute old, a refresh age
  nobody declared; its time could stand up to that minute behind the attempt it reported. Every
  attempt is now written, so the record is as fresh as the worker's declared poll period makes it.
  A previous record that cannot be read is no longer read as absent in silence: the worker says on
  stderr where it is and why it cannot be read, then counts any run of refusals from the attempt
  it writes.

- **The submit queue keeps a gate's whole report and a reviewer's whole answer:** a shard's record
  kept the last 60 lines of what its gate printed and a review the last 40 lines of what the
  reviewer said, numbers nobody stated, so a compiler error above its warnings or a finding above
  the verdict was cut off and a refused candidate's refusal did not say why. Both are now kept
  whole; the review still leaves out the wait mechanism's lines, which say what it waited on, not
  what it found.

- **A checkout whose origin's default branch was deleted and pruned is still read as canonical:** a
  clone records the default branch it saw (`origin/HEAD`), and once that branch was deleted on the
  origin the record named a branch that no longer exists. Stado read it as the branch the checkout
  must work on, so a checkout on the origin's real default was refused as "not on <deleted branch>"
  and `stado quality hooks` could not install its push gate. A recorded `origin/HEAD` is now read
  only while the branch it names is a remote-tracking branch of the checkout; otherwise the origin
  is asked for its default.

- **`stado host run attach` says what to run instead when it is handed an installed program:**
  attach runs only a program delivered into a run of its own below `$HOME/.stado/work/runs`, and
  handed the host's installed `~/.stado/bin/stado` it answered only `path … must be an absolute
  path below the target account's $HOME/.stado/work/runs`, so a caller who wanted that Stado to
  install an archive retried the same call. The refusal now carries a line saying why attach never
  drives an installed program (the bytes it starts are the ones a run's delivery placed), naming
  the commands that do the job: `stado host run deliver <target> <file>
  .stado/work/runs/<id>/<name>` then attach that path, `stado host upgrade <target>` to bring the
  installed Stado to a newer revision, and `stado host exec <target>` for its approved reads. With
  `--json` that line is the error envelope's `help`.

- **A catalog that cannot be read holds a submit-queue candidate instead of refusing it:** the
  pass read the product's catalog record and submit policy on the candidate's behalf and refused
  the candidate when it could not, so one catalog commit that did not validate refused every
  queued candidate of every product and each submitter had to submit again. Such a candidate now
  stays queued with `waits_on: declaration`, its refusal naming the catalog and its error, the
  pass carries the candidates of other products behind it, and the first pass after the catalog
  reads judges it. `stado change list` and `status` say `waits on: a declaration` with how to
  lift it.

- **`stado release staged activate` takes the settle window from the operator and reports the
  installer's whole run:** the activation slept 30 seconds before reading the runtime link again
  and printed the last 6 lines of the installer's log, both numbers nobody stated. The window is
  now the required `--settle-seconds` (the host's restoring service's cycle, which Stado cannot
  read), refused by name with exit 2 when missing or zero, and Stado Desktop's **Activate a
  verified staged release** form requires it too. The report carries every line the installer
  wrote to its log during this run, says when it wrote none or no log exists, and gives the whole
  log when the installer created it or replaced it (a different file, known by its inode, even a
  longer one); a reverted link names the window it was reverted within.

- **A resolver no longer holds every client for a host none of whose SSH paths answers:** each
  connection to a service placed on such a host tried every path again, so a service on a host
  that went away held each client for as long as its paths took to fail, and the caller gave up
  first. The resolver now remembers, per active host, that its last session attempt was refused
  by every path and why, and refuses a connection to that host within the refresh interval at
  once with an HTTP 502 naming that refusal (`not tried again: every SSH path of <host> refused
  <N>s ago, within this resolver's <N>s refresh interval (…)`). The first connection after the
  interval tries again; a session that opens forgets it. Real journey:
  `tests/resolver/unreached.rs`.

- **A CLI that carries a resource bundle passes readiness:** a CLI stage places a directory under
  `bin/` beside its commands (a SwiftPM `<Package>_<Target>.bundle` that `Bundle.module` reads)
  whole, with no link on PATH, but readiness judged every recorded path as an executable, so the
  bundle failed as `<bundle> is not executable` and every installation of such a CLI was rolled
  back. A placed directory is now passed over; a recorded path that is missing or unreadable is
  still refused as not executable.

- **Azure VM creation is decided by ARM's `error.code`, not by words in its messages:** a NIC or
  VM create refusal was classified by searching the message for `QuotaExceeded`,
  `OperationNotAllowed` or `SkuNotAvailable`, so a message that quoted one of them for another
  reason skipped a location and one that phrased it differently did not. A refusal ARM states is
  now kept with its HTTP status and `error.code`, from the response and from a long-running
  operation that ends Failed or Canceled, and a location is skipped only for those codes. Any other
  refusal is no longer dropped: when no location places the VM, the create fails naming each
  location's refusal. A NIC that cannot be removed after a refused VM, or after a delete, fails the
  call naming the VM, the NIC and both refusals instead of being logged and left holding the name
  the next attempt needs.

- **An Azure endpoint on an IPv6 link-local address is admitted like an IPv4 one:** a declared
  `azure.cloud.*` endpoint on `fe80::/10` was refused while `169.254.0.0/16` was admitted, though
  both are link-local; both now are.

- **One tick at a time runs over a store:** every coordinator tick now holds
  `state/coordinator/tick-lock.json` naming its process while it runs, so two ticks over one store
  no longer assign, dispatch and reap the same jobs twice. A record whose process on this host is
  gone is taken over; one still running, or held on another host, refuses the tick naming its pid,
  host and start. A resident coordinator that finds the store held logs `tick skipped: …` and
  ticks at its next interval.

- **A push to a mirror or a fork is no longer checked as one to origin:** the pre-push hook
  dropped the remote name and URL Git hands every pre-push hook, so `stado quality check
  --pre-push` took every push of `main` for one to the `main` the submit queue lands on, and a
  commit pushed to a backup mirror was refused for what only the shared `main` must hold. The hook
  now hands them on as `--remote` and `--remote-url`. A push Git names as `origin` is always
  checked, whatever its URL reads; any other remote is passed, with a line naming both
  repositories, only when its URL names a repository none of `origin`'s push URLs names (host
  and path, with scheme, user, port, `.git`, a trailing `/` and case set aside). With no remote
  given, a URL that names none, or no `origin`, the push is checked as before and the check says
  why. A hook an older Stado wrote reads as `outdated` until `stado quality hooks install`
  rewrites it. The push journey is now its own target, `quality-push`.

### Added

- **A push cannot add a call to a product the catalog does not show it running without:**
  `stado product dependencies` reported every product's code that reaches another catalog
  product with no declared alternative, but nothing refused a revision that added one, so new
  coupling was found only by the failures it caused. `stado quality check`, which every pre-push,
  change hand-off, release submission and delivery gate runs, now judges the lines a revision adds
  against its published `origin/main` exactly as `dependencies` judges a whole checkout, and
  refuses a reach that is undeclared, bound, has no `alternative_source`, or names one the
  workspace does not hold, naming each file and line and the `stado product registry set
  --add-dependency` that declares it. A line already on main is not a later push's refusal. A
  product the catalog does not record, and Stado, the catalog's authority, are not judged, and the
  check says so naming why. Every verdict names the catalog it read and at which commit, read as
  every catalog reader reads it (a checkout behind its published main at `origin/main`). Real
  journey: `tests/quality/independence.mjs`.

- **A placement profile can be taken from a host that does not answer:** `stado placement move`
  stops the source's units and copies its states over the source's channel, so a profile placed
  on a host that is switched off or off the network could not leave it: every step stood until
  its caller gave up, and every consumer of its services waited on a host nobody reached. `stado
  placement take SERVICE... --reason WHY [--check] [--json]`, run on the host that takes it,
  probes each SSH path of the source once and is refused while one answers (then the profile
  moves with its states). SSH closed is not the units stopped, so it then asks each probe URL the
  profile declares for the source at every name the source is known by, within the resolver's
  refresh interval, and is refused while any answers (`<source> still serves <service> at
  <url>`), since a copy started here would serve beside it and write diverging state. It is
  refused when this host lacks a state the profile requires, when a routing unit sits on the
  source, and for the fleet vault. Otherwise the profile's units start here, every probe must
  answer 2xx, and one registry commit places the profile here, the same change a move commits
  (now one function both share). The receipt gives the age of this host's copy of each state
  (every write the source took after it is lost), the optional states not recovered and the
  source's units left running, to retire before it serves again; it is kept in
  `placement history` as `take`. Stado Desktop runs it and its check. Real journey:
  `tests/placement/take_profile.rs`.

- **A product's submit policy is declared through `stado product registry set --submit`:** the
  submit queue refuses every change of a product whose catalog record declares no `submit`
  policy, naming the declaration it needs, but no verb could write one, so such a product's
  changes had no queue path. `--submit` takes the whole declaration as one JSON object
  (`criticality`, `max_changed_lines`, `low_risk_max_changed_lines`, `review_paths`) and replaces
  the record's `submit`; an unknown field is refused by name, and the catalog validation every
  write runs refuses a standard product without a low-risk limit, a critical one with it, a zero
  limit, an unknown criticality and a review path outside the repository. Stado Desktop's "Update
  a catalog product" form carries the same field.

- **The Azure cloud is declared, not built in:** the Resource Manager, the OAuth authority, the
  managed-identity endpoint and the Retail Prices API were constants of the public cloud,
  scattered through VM dispatch, the credit gate, sign-in, role repair, billing, the SKU catalog
  and quota replies. They are now the compute bindings `azure.cloud.resource_manager`,
  `azure.cloud.authority`, `azure.cloud.identity_endpoint` and `azure.cloud.retail_prices`
  (`AZURE_RESOURCE_MANAGER`, `AZURE_AUTHORITY`, `AZURE_IDENTITY_ENDPOINT`, `AZURE_RETAIL_PRICES`),
  each the public cloud's when undeclared, so a deployment can rent from a sovereign cloud and a
  listener on a loopback address can stand in for Azure. A declared endpoint that is neither
  HTTPS nor on a loopback or link-local address refuses every Azure call naming the binding, and
  `stado config show` names the endpoints in force. The ARM client no longer has an
  unauthenticated mode: every request carries the token.

- **`stado scheduler tick`: one coordinator tick, now:** the tick that settles cancellations,
  fires due schedules, reaps leases, assigns and dispatches ran only inside a coordinator's loop,
  so nothing placed a queued job or fired a schedule before the next interval, and nothing could
  observe one tick end to end. `stado scheduler tick [--provider P]... [--billing] [--json]` runs
  the same tick once in this process, holding the store's tick lock, and answers how many machines
  it started and which arms ran; a provider that is not a compute provider, is fenced or not
  enabled, or has no tick arm, and a store another tick holds, are refused before the tick.
