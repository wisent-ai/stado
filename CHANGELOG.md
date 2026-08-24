# Changelog

All user-visible Stado changes are recorded here. Stado follows Semantic Versioning and the release, compatibility, migration, and rollback contract in [`docs/release.md`](docs/release.md).

## Unreleased

### Configuration

- Removed Wisent's deployment profiles and bundled fleet registry from the
  public repository. Stado now ships an empty registry, `stado config init`
  creates the neutral local starting point, and coordinator installation
  requires an explicit operator-owned `STADO_CONFIG`.

### Desktop

- Restored the local control plane as Stado Desktop's default source, removed
  mandatory deployment setup from the local path, and made the source visible
  in Settings.
- Kept the local operations console available without a Wisent session, moved
  account sign-in to remote deployment actions, and made the menu-bar app
  reopen the native console instead of sending users to a browser.
- Corrected dashboard state decoding and status presentation so worker,
  capacity, job, failure, and onboarding labels reflect the published backend
  snapshot instead of optimistic placeholders.
- Separated HTTPS proxy trust from deployment RLS identity: the local profile
  no longer carries an invalid non-UUID deployment binding, direct loopback
  dashboard access needs no Supabase round trip, and proxied product APIs keep
  their credential boundaries.
- Made the macOS bundle use its repository-owned canonical app icon without
  depending on a nonexistent asset resolver.
- Added an Add a Machine path to Stado Desktop's Hosts screen, reachable from
  the context bar and from the empty registry state. It walks naming, key
  minting with the public half and its authorized_keys line to carry,
  the SSH address, verified enrollment, and the channel and agent proofs;
  every step runs one allowlisted `fleet` or `host` command through the
  dashboard's `POST /api/operator/run` argv bridge, never a shell string.
  Progress survives closing the window, so the walk to the other machine
  does not cost the minted key, and an enrollment that fails says whether it
  never reached the machine or reached it and rolled its own entry back.
- Stado Desktop's Add a Machine window now opens on the ways in rather than on
  one of them. The list comes from `stado fleet methods --json`, so a method
  this fleet's registry catalog forbids is shown disabled, naming the field
  that forbade it, instead of being missing or dead. Invite mints a code, shows
  it once with the single line to send to whoever has the machine, then waits:
  the wait survives quitting the app, and reopening the window says which
  machine is expected, what it reported when it answers, and offers approve or
  reject. Adopt takes an address and installs the fleet's public key over a
  session the control plane can already open, and says plainly that no password
  can be typed into the window because the process opening that session has no
  terminal. Join and Declare each have a screen stating what the operator has
  to do, with working pending and approve behind Join. The old path — key
  minted here, installed by hand over there — remains for the machine nobody
  can reach. Every call still goes through `POST /api/operator/run` with a
  `fleet` argv array and the mutation confirmation, and the two proofs now run
  for a machine added by any method rather than only for the hand-carried key.
- Stado Desktop reads a command's payload before it reads its exit status.
  `host gates` exits non-zero when a host is claiming nothing, `service
  converge` when a binary has drifted and `release status` when a host never
  reported its software — each after printing the complete `--json` answer —
  and the console turned all three into an error pane on the exact screen built
  to show them. The Hosts screen said "3 hosts did not answer whether they are
  claiming work" while the CLI was answering for every one of them. The exit
  status is still what a failed read reports, but only when there is no payload
  to read.
- The Releases screen lists a host's quarantined digests with the one the
  registry desires first and the rest newest first. The host answers in digest
  order, which put the digest actually blocking brama on control-host
  seventh of seven, below six refusals that were already history.
- Documented the Releases, Hosts and Services screens in the README and the
  onboarding guide with screenshots of the live fleet, under
  `desktop/StadoDesktop/docs/screenshots/`.

### Host operations

- Added service reconciliation to every autonomy cycle. Stado now turns stale
  service beacons into `unknown`, joins fresh unit state with a current
  consumer-side endpoint sweep, safely adopts only a loaded unit whose live
  program matches its declaration, idempotently ensures a unit only after the
  endpoint is proven unreachable, and persists each run with deduplicated
  failure alerts, mutation leases, action limits and circuit-breaker feedback.

- `stado host link TARGET [--json]` now says whether anybody is logged in on
  that host's screen. An operator asking where to see this in the GUI was
  answered "nowhere": the fact existed only inside `stado service restart`,
  which had to write to a host that is already the wrong shape before it would
  print `launchd_domain: {status: fallback}` and the sentence underneath it.
  The same resolver is now also a read-only probe, and the document carries
  `session: {kind, console_owner, detail}` — `headless` with `console_owner:
  root` on `control-host`, `graphical` with the login's own name on a mac
  somebody is sitting at, and `unknown`, never a guess, when the read did not
  land. `detail` is the resolver's own sentence, unabridged: `/dev/console
  belongs to root, not charles: no graphical session, so gui/501 does not exist
  and a LaunchAgent has only the background domain user/501`. A headless host
  is not by itself unhealthy and the verdict rules are unchanged. A headless
  host that also declares a per-login unit gets one blocker per unit, in the
  words the question was asked in — `nobody is logged in on the screen here,
  and <unit> is registered as a user service, so this machine cannot start it`
  — followed by the one privileged command that installs it where the machine
  can load it. That is the chain nothing in this product could state: no
  session, so no per-login domain, so the unit cannot be loaded, so the host
  publishes no capacity, so the job pinned to it waits.
- Added `stado host software [TARGET] [--json]`: what a host actually runs, one
  row per program with its version, its SHA-256 and whether those exact bytes
  came out of a release Stado published. Every other read in the pack asks about
  a declaration — `managed_versions` says which version a host must run,
  `service list` says a unit is loaded, `release status` says what the registry
  desires — and all of them stay true across a release that never reached the
  box. On 2026-08-18 two macs were running a skarbiec built on a laptop, 0.2.1
  on one and 0.2.3 on the other, neither in any published release and both
  declared 0.1.3, and the older one stripped the `brama:agent:<id>` tags off a
  live credential every rotation for a day while no screen in the fleet could
  name the program doing it. `provenance` is decided by digest on the host and by
  nothing else: `host release` stages every delivery out of an archive whose
  SHA-256 it verified against the canonical release manifest and hard-links that
  staged file into place, so bytes matching a staged artefact provably came
  through the channel and bytes matching none provably did not — a name, a
  version string and a program's own claim about its provenance all survive one
  `scp`, and that digest does not. The population is every program in
  `$HOME/.stado/bin`, every declared unit's program, and every release-control
  product install path, the last of which appears in neither of the others.
  Shell scripts are counted rather than rowed, because control-host carries
  1393 of them against 28 programs and a release pipeline produces none. The
  report is stored as an observation in `~/.stado/observations.json`, so it
  carries an age and goes stale, and a read that fails is recorded as
  `unverified` instead of leaving yesterday's answer looking current. The
  reporter is a checked-in script embedded in the binary and run over the same
  audited fixed-script channel `host provenance` reads with; nothing is
  installed on the host.
- Added `stado host retag-vault-item TARGET ITEM --tags a,b,c`: an owner-only
  vault write that replaces an item's tags and never its payload, over the
  fixed-script channel. A vault item's tags are the only thing binding a
  subscription credential to its agent — `list_subscriptions(agent)` discovers
  accounts by the `brama:agent:<id>` tag — so an item whose tags were stripped
  leaves the fleet while the credential itself stays valid. This is what brought
  the kimi subscription back: five tags restored on
  `provider:kimi:brama-sub-wisent-app-kimi-primary`, revision unchanged at 144
  because the payload is untouched, and a signed request for
  `kimi/kimi-k2-thinking` through the always-on gateway then answered 200.
- Removed `stado host install-helper` and `stado host run-helper`. The helper
  channel — deliver a checked-in script into `$HOME/.stado/bin`, then execute it
  by name — was the source of recurring ad-hoc-script damage on hosts, and
  everything stado itself asked through it now travels inside the binary: the
  service-endpoint probe (`service verify`), the Apple-account-holders probe
  (`identity verify`), the placement-policy apply pass (`host
  publish-placement-policy`), the installed-version reporter (`service
  converge`), and the leftover inventory (`host helpers`) are embedded
  fixed scripts run over the same audited channel, so there is nothing to
  install on a host and nothing left behind. `host remove-helper` and
  `host helpers --prune` remain to reap what the channel already delivered;
  `host install-file`, `host install-secret`, and the allowlisted `host exec`
  are the surviving delivery and read channels.
- Added `stado host sync-acquisition-scopes TARGET SOURCE`, the reviewed
  replacement for running weles's acquisition-scope register script through
  the retired helper channel. The catalog travels through the `host
  install-file` delivery path into `$HOME/.stado/files`, and an embedded
  fixed script derives the Ed25519 workload public key on the host —
  migrating an older key only after registration with its successor
  succeeded — and registers the catalog against the host's fleet vault with
  `skarbiec token-register-acquisitions --replace-capabilities`, printing
  the reconciled status. The old script's two appstore `token-mint` calls
  are not carried over: they mint unrelated `weles-worker` credentials, and
  each call silently extended those tokens' expiry.
- `stado host gates <host>` answers "why is this host claiming nothing" in one
  payload. It joins the blockers the host's own queue agent publishes with the
  disk policy behind them and the slots the registry declares, and reports the
  agent's words verbatim. The Mac mini sat at roughly 2 GiB free against a
  55 GiB policy, published `disk_pressure_unresolved` every tick, failed
  admission closed and claimed nothing for hours while every release build
  queued behind it — and no command said so, because the one fact that mattered
  lived only in its capacity broadcast. Read-only, safe against a live host, and
  it exits non-zero when the host is not claiming.
- `stado host reclaim <host>` reclaims the space in declared stages — the
  host's own janitor pass, the release build scratch tree, and delivered product
  trees no `current` link and no live process references — measuring free space
  and counting items either side of each one. It previews by default; `--apply`
  is the only thing that deletes, it refuses to run without `--reason`, and the
  reason is appended to an audit log on the host whose disk changed. Nothing
  outside those declared roots is touched, nothing a live process holds is
  removed, and the newest tree of a product is always kept. This replaces the
  hand-written ssh script the outage was actually settled with.
- `stado host disk --json` now also reports the low watermark the host's janitor
  last validated, which is the threshold admission is really gated on when a
  host cannot read the registry.
- The disk janitor gained a `chromium_clones` cleaner, and `stado host reclaim`
  the matching fourth stage: macOS clones the whole Chromium bundle on every
  launch to validate its signature, Weles drives Chromium for browser
  automation, and a killed run leaves its clone behind. The mini carried 137 of
  them, 130 untouched for more than a day, while its queue agent published
  `disk_pressure_unresolved` and every release build queued behind it — and
  nothing in the product removed or even reported one. Only entries macOS
  itself named are candidates, the newest clone in the root is kept whatever
  its age, nothing younger than the policy's minimum age (a day, floored by the
  registry) is touched, and no clone a live process names in its argv is taken.
- `stado host reclaim`'s `delivered_trees` stage now also sweeps every
  superseded delivery root the product catalog declares
  (`superseded_roots` in `stado-rs/data/products.json`), under the rules it
  already applied: at least a day old, never `current`, never a product's newest
  tree, never a path a live process names. The mini holds 20 inert
  `weles-worker` versions (9.7 GiB) under `$HOME/.local/share/weles-worker` from
  the installer that predates the artifact install path, which no rollback will
  reach and no command could report. A product's live install root is never
  swept.
- `stado host disk` names the host's local APFS snapshots, and `stado host
  gates` adds a `local_snapshots_unreclaimable` note — a note, never a blocker,
  so it cannot change the claiming verdict or the exit status — while the disk is
  the reason a host claims nothing. Their blocks are inside the `used` figure,
  no stado command removes them, and macOS publishes no size for a snapshot, so
  the count and the host's own names are reported and no byte figure is invented.
  An operator is no longer left believing a reclamation that freed less than the
  deficit means the numbers are lying.
- Removed `stado-rs/scripts/reclaim-mini-disk-host.sh`. Every stage it had is a
  product command now: `stado host reclaim <host> [--dry-run|--apply --reason]`.

### Release control

- Stado 0.7.9 is the first release the pipeline carried end to end onto its
  own fleet: both platforms built, gated, signed, and published, and every
  host installed the release's own bytes. Deliveries are pinned to the
  registry target they install on (new optional delivery `target` field) and
  run `stado release install-local`, so no delivery needs ssh or a login
  service anywhere.
- All 137 operator python scripts were retired on the operator's order. The
  one load-bearing script — the fleet delivery installer — became
  `stado release install-local`; the version-check and deploy workflows still
  name `scripts/surface.py` and `scripts/baseline.py` and stay red until that
  gate's logic is ported, deliberately unremoved.
- A resumed run retries failed deliveries instead of completing past them,
  a running platform leg reports crates compiled against the previous run in
  the CLI and the Releases screen, job inputs naming `stado://releases/...`
  read the public release channel instead of the job store's namespace, the
  release worker resolves programs from `~/.stado/bin` and `~/.local/bin`,
  and `host reclaim` gained `queue_workdirs` (terminal job workdirs, keyed on
  the live queue set) and `foreign_home_trees` (macOS-style `/Users` debris
  on Linux hosts) stages.
- `stado release status` no longer exits zero on silence. It printed
  `brama target=control-host desired=0.2.27 observed=unreported` and
  succeeded, which made a host that had never once said what it runs
  indistinguishable from a healthy one in the command an operator reaches for to
  ask exactly that. Each target row now also carries the host's own software
  report (`stado host software`), read out of the observation store rather than
  gathered per target, and the command exits non-zero when a target has no
  report, a report older than the observation TTL, a refused read, an
  `unmanaged` program, or a version disagreeing with what the fleet declares —
  naming the host and the exact disagreement in one sentence per row. The gate's
  scope is what the fleet declares it manages: every name in the target's
  `managed_versions` plus the release-control product's own binary at
  `<install_root>/<binary>`, which lives under the product install root and
  appears in no `managed_versions` entry. A program nothing declares — a host's
  `$HOME/.stado/bin` accumulates dated backup copies, eleven of `stado` on one
  laptop, none of them running — is reported and counted but does not fail the
  gate, for the reason `service converge` refuses to fail on an unmeasured
  binary: a command that fails forever on fossils teaches operators to append
  `|| true`, and then the drift it exists to catch stops being noticed again.
  StadoDesktop's Releases screen shows the same verdict in a `Software` column,
  pulls a failing row up beside the blocked ones, and lists the CLI's sentences
  verbatim — it reads `verdict`, `failed` and `findings` out of `status --json`
  and re-derives none of them, so one command decides what `unmanaged` means.
- A failed release job now reports its own last words: `release submit` reads
  the job's output-log tail from the store and carries it in the error, the
  failing platform and pinned host named in the same sentence, and persists it
  in the run object. `stado release status` lists the newest pipeline runs
  with the first line of each persisted failure, and Stado Desktop gained a
  System > Releases screen that shows the same text — one source for the CLI,
  the web operator console, and the app.
- The release worker names each step before it runs it, resolves gate and
  build programs where the agent host actually keeps them, and provisions the
  pinned toolchain's own gate components (rustfmt, clippy) idempotently
  instead of dying with a nameless "No such file or directory" under a
  LaunchAgent's minimal PATH.
- The stado release recipe builds into WISENT_OUTPUT_DIR, where its own stage
  map has always claimed the artifact lives.
- Added repository-owned Stado release manifests, immutable source inputs,
  signed build and delivery receipts, and provider-specific delivery adapters.
- Added fleet-wide product catalog ownership, retry-safe release submission,
  canonical promotion, exact-digest host reconciliation, and blue-green
  rollback state.
- Release-managed runtimes now receive their immutable product, version,
  platform, and artifact digest identity in the process environment.
- `stado release logs PRODUCT --target TARGET` reads a candidate's own stdout
  and stderr off the host — the `{logs_root}/{product}-{version}.{out,err}`
  files the release agent opens for every candidate it spawns. A brama
  candidate died in under ninety seconds and the rollout state said only
  `candidate did not become ready within 90s: pid 46748 is gone`, while the
  process's own account of its exit sat unread in
  `<logs_root>/brama-0.2.27.err`; reading it took a hand-typed ssh session.
  Defaults to both streams and the last 40 lines, reports the whole file's
  size next to the tail, and distinguishes a log that is missing from one that
  is present and empty.
- `stado release doctor PRODUCT` answers "will this rollout land, and if not,
  what is holding it" in one read-only pass: desired versus observed release,
  the rollout phase and detail, the candidate's port, liveness and readiness
  answer, the host's quarantine map with the desired digest called out, and the
  host's claiming gates. The verdict is `blocked` when the desired digest is
  quarantined — which the agent skips silently on every pass — or when the
  host's disk gate is unresolved, the state in which the queue agent claims
  nothing; `rolling` while a candidate is in flight; `settled` only when
  observed equals desired. A gate that cannot be read fails the command
  instead of being assumed healthy.
- `stado release quarantine list PRODUCT` shows the digests a host refuses to
  roll out again with the agent's own reason for each and the desired digest
  called out, and `stado release quarantine clear PRODUCT --target HOST
  --digest SHA256 --reason TEXT` retires exactly one of them. Retrying a
  quarantined digest previously required hand-editing
  `{state_dir}/<product>.json` on the host or burning a version number to
  change the digest; there was no command. Clearing starts, stops and restarts
  nothing — it removes one map entry and the agent rolls the digest out on its
  next tick. Both `--digest` and `--reason` are required, the previous state is
  copied to a timestamped backup beside it, the rewrite is refused if another
  writer moved the file or if the document does not hash correctly after
  landing on the host's disk, and every clear appends an actor-stamped line to
  `{state_dir}/<product>.quarantine-audit.jsonl`.

### Onboarding platform

- Completed the invite screen against today's CLI. It decodes `base_source`,
  `base_is_temporary` and `base_warning` and shows the control plane's own
  temporariness sentence verbatim beside the one line; a new entrance section
  reads `fleet ingress status --json`, names the standing address and its
  lifetime, offers `ingress up`/`ingress down` from the window, and — when
  nothing serves the line and no `enrollment.url` is configured — says so and
  offers the one button that changes it, instead of letting the mint fall to
  offline as a surprise. Long bridge commands stopped dying at the transport:
  the client's per-request timeout now clears the 300 s command ceiling, which
  `fleet ingress up` and `fleet enroll --bootstrap` legitimately need.
- Made the operator bridge's children see the store their parent serves. The
  dashboard is the one process that reads the disk store directly and its
  launcher says so with `WC_STORAGE_BACKEND=local` — but a CLI child spawned
  by `/api/operator/run` inherited that override and read bare paths at the
  store root, while the same parent served every remote writer namespaced
  blobs. Through the bridge, `fleet invites` answered "no invites" for
  invitations sitting in the store it was served from — two views of one
  disk. The child now drops the override and resolves storage from the config
  file like any client; observed agreeing afterwards on the always-on host.
- Made enrollment objects writable in this fleet's store. Everything the
  enrollment methods record — invites, machine-filed join requests, the
  published ingress address — lives under `enrollments/` inside the queue's
  own namespace, the object API authorizes writes per declared prefix, and
  `enrollments/` was never declared: every write, including the pre-existing
  `stado fleet join` path that nobody had ever exercised, answered
  `401 unauthorized or non-immutable release write`.
  `scripts/allow-enrollments-prefix.py` adds the prefix idempotently and
  atomically to a host's control-plane config; the object API on the always-on
  host was adopted into the registry (it was live and undeclared, under a
  LaunchDaemon a never-loaded duplicate LaunchAgent shadowed) and restarted in
  place through `stado service restart`. A production invite now records,
  lists, revokes, and cleans up end to end.
- Made a consumer grant recoverable after a credential item is removed. Grants
  only ever union, so capabilities naming a deleted item stayed forever, and
  the next widening re-mint was refused with `capability names a missing
  item` — one removed test key froze every future grant for
  `local-operator`. `scripts/scrub-consumer-grant.py` re-mints with the same
  list minus capabilities whose item is gone or in trash, bearer and TTL
  preserved, vault copied first, idempotent.
- Added read-only recon helpers `scripts/report-object-api-host.py` and
  `scripts/report-object-api-launcher.py`: which process holds the object API
  port, which launchd declaration owns it, what its launcher resolves at
  startup, and whether the namespace policy already carries `enrollments/` —
  the model a restart of an always-on control-plane process must be preceded
  by, written down as an instrument instead of a lesson.
- Made the one-line `invite` mode publishable without publishing the operator
  plane with it. `stado dashboard --enrollment-only` runs a listener that
  serves exactly `GET /join.sh`, `GET /api/fleet/invite/key` and
  `POST /api/fleet/join`; every other path and every other method on those
  paths answers `404` with one mute body, decided before the Host guard,
  before any authorization, and before the object store or the credential
  store is touched. That is an allowlist rather than a list of surfaces to
  hide, so a route added later is unreachable in this mode until somebody
  names it — the reverse of the failure where a new route is published by
  default and nobody notices. Pointing a tunnel at the full dashboard would
  have published `POST /api/operator/run`, which executes catalog `stado`
  commands for any caller the loopback bind makes trusted; this mode cannot
  reach it. The narrow listener also starts none of what the refused routes
  need: no Skarbiec boundary verifiers, so it runs where no vault does, and no
  queue-refresh loop. Its startup log names the three served pairs, since that
  is what the operator is about to expose.
- Added `enrollment.url` (`STADO_ENROLLMENT_URL`), the origin `stado fleet
  invite` probes and prints its one line from. It is empty by default and
  falls back to `api.url`, so an unconfigured deployment behaves exactly as
  before and both empty still reports `not_configured`. It is a separate key
  because `api.url` is the release and deployment endpoint that self-update,
  remote bootstrap, cloud-agent dispatch and the coordinator resolve through:
  repointing it at a narrow enrollment listener would break all of them.
  `stado config show` reports it as `enrollment_url`.
- Added `stado fleet ingress up|status|down`, which turns the one-line `invite`
  mode from a documented precondition into a command. `up` picks a free
  loopback port, starts `stado dashboard --enrollment-only` on it, starts a
  Cloudflare **quick** tunnel in front of it — no Cloudflare account, no API
  token, no zone, no DNS record — and then fetches `/join.sh` back through the
  public `*.trycloudflare.com` address **from the internet**, requiring `200`
  and the same byte count as the script this build serves. Only that match
  publishes `enrollments/ingress.json` (`base_url`, `mode`, `host`,
  `started_at`, `verified_at`, `listener_port`, `pid_hint`); a failure at any
  earlier stage stops both processes and names the stage, so there is no state
  in which an operator is told an entrance exists and it does not. Both
  processes are started as process-group leaders and outlive the command, and
  `down` signals the groups, so nothing they spawned keeps the port; a pid
  whose command line no longer matches is reported as gone rather than
  signalled, and an object recorded by another machine is refused instead of
  acted on. `--port` on a port already in use is refused before anything
  starts: this command never puts a public tunnel in front of a service it did
  not open. Cloudflare documents quick tunnels as non-production and rate
  limits them, and their address changes on every start — both are printed by
  `up` and by `status` rather than left in the documentation. `--named` is
  refused in one sentence, because the vault has no
  `platform-admin-cloudflare#api_token` field and Skarbiec will not grant on a
  field that does not exist.
  Two steps inside `up` earn their place. The tunnel is started with
  `--http-host-header 127.0.0.1:<port>`, because the dashboard's DNS-rebinding
  guard answers `403` to a forwarded `Host: <name>.trycloudflare.com` and the
  honest fix is for the proxy to present the authority it is actually
  connecting to, not for the guard to be relaxed. And the wait for DNS goes to
  Cloudflare's own DNS-over-HTTPS resolver rather than to this machine's: the
  record appears a few seconds after the address is printed, and a local lookup
  in that window leaves an `NXDOMAIN` in the negative cache — measured here, one
  premature lookup made the address unresolvable for 64 seconds after
  Cloudflare had already published it, and where the zone's negative TTL is
  honoured that is 1800 seconds.
- `stado fleet invite` now takes its base address from `enrollment.url`, then
  from the published ingress **while that address still answers**, then from
  `api.url`. A one-liner built on an ingress address says out loud that it is a
  temporary tunnel, that it dies with the ingress, and that a restarted ingress
  returns under a different address; `--json` adds `base_source`,
  `base_is_temporary` and, for a tunnel base, `base_warning`. `--offline`
  consults no ingress and its probe verdicts are unchanged.
- Added product-scoped delivery for immutable onboarding bundles, sticky
  experiment assignment, canonical event collection, and attempt-state reads.
- Added Stado Desktop's product-owned first-use journey and gated completion on
  a real authorized job result rather than deployment or setup navigation.
- Replaced the Oko-specific onboarding relay with the same closed,
  least-privilege operation contract used by every registered product client.
- Corrected and completed the machine-onboarding documentation. Every
  documented `stado registry host add` invocation now carries the required
  `--release-platform` alongside `--ssh`; the previously published form failed
  on use.
- Documented enrollment as the verified path it is: `stado fleet key generate`
  prints the public key that first contact needs, `stado fleet key install`
  travels through the existing channel and is therefore rotation rather than
  first contact, and `stado fleet enroll` probes `hostname` and `uname` before
  it writes and rolls the entry back when bootstrap fails. Added the
  `stado fleet` family to the CLI reference and recorded Stado Desktop's
  equivalent Add-a-Machine surface; `stado_fleet` is documented only as a
  compatibility binary.
- Documented that the SSH destination may be any reachable target — a `.local`
  name on the local network is as valid as a tailnet name — and what a `.local`
  destination costs: channel-opening commands then require the same network,
  while the outward health beacon keeps `stado registry beacon-age` reporting.
- Added [Add your own machine](docs/add-your-machine.md) for the owner of a
  machine joining someone else's fleet, linked from the README next to the
  operator onboarding path.
- Added `deploy/join.sh`, the one-line bootstrap the owner of a joining machine
  runs for the `invite` method: `curl -fsSL <control-url>/join.sh | sh -s --
  <code>`. It redeems the invitation for the fleet's public half, installs that
  line into `~/.ssh/authorized_keys` idempotently with 700/600 modes, resolves
  the address the fleet should dial (tailnet name, then `.local`, then the
  default interface's IPv4), and reports the machine as a pending enrollment.
  It never handles a private key, never prints or stores the invitation code,
  never enables Remote Login silently — it diagnoses SSH and prints the exact
  macOS or Linux step for the owner — and deliberately installs no agent, since
  the operator's `stado fleet approve` does that over the channel it just
  opened.
- Rewrote the machine-adding documentation around the four named methods
  instead of one procedure: [Onboard another machine](docs/onboarding.md#onboard-another-machine)
  now opens with a chooser table for `invite`, `adopt`, `join` and `declare` —
  what each needs from the operator, what it needs from the machine, when it is
  the right one, and what it cannot do — and gives each method its own section.
  Every method states the same checkable property: the private half of the
  channel key never leaves the operator's credential store, and the machine
  receives only the public line.
- [Add your own machine](docs/add-your-machine.md) now leads with the one line
  the owner of a joining machine actually runs, because the invitation is the
  normal path; pasting a key by hand is kept below as the route for a machine
  that cannot run it, next to the operator-driven `adopt` alternative.
- Documented in the CLI reference: `stado fleet methods`, `stado fleet invite`,
  `stado fleet invites`, `stado fleet revoke-invite`, `--install-key` on
  `stado fleet enroll`, `--json` on `stado fleet pending`, the `allow_invite`
  and `allow_adopt` catalog fields, and the three invite routes
  (`GET /api/fleet/invite/key`, `POST /api/fleet/join`, `GET /join.sh`) —
  authorized by an invitation token alone, and unable to write the registry,
  which stays an operator-authority write inside `stado fleet approve`.
- Added [`docs/examples/fleet/invite-a-machine.sh`](docs/examples/fleet/invite-a-machine.sh),
  the `invite` method end to end from the operator's side, from `fleet invite`
  to `fleet approve` and `registry beacon-age` as the proof, indexed in the
  examples README.
- Corrected the `invite` method's documentation, which described a path nobody
  outside the tailnet could walk: it printed
  `curl -fsSL https://stado.wisent.com/join.sh | sh -s -- <code>` as the
  machine owner's whole part, while that name is in no DNS zone and the control
  API listens on loopback only. The method is now documented as the two modes it
  has. [Onboard another machine](docs/onboarding.md#onboard-another-machine)
  gives each mode its own section, the chooser table distinguishes them, and the
  one-line mode carries the three things that have to exist before it can work —
  a name that resolves, an ingress in front of the loopback-bound dashboard, and
  a release there serving the invitation routes — stated as requirements rather
  than as something already published.
- Documented the offline mode as the way a machine is added today: the operator
  sends the fragment `stado fleet invite --offline` prints, its owner pastes it
  on the machine being added, and the `user@address` it prints on its last line
  comes back for `stado fleet enroll NAME --ssh ADDRESS --bootstrap` to close.
  The fragment carries only the fleet's public key and is documented as not
  being a secret, so it travels by whatever channel already reaches its owner.
- [Add your own machine](docs/add-your-machine.md) now opens with that fragment,
  because it is the part that works, and keeps the one-line `curl` form lower
  down with the reachability it requires and the reason a fragment arrives
  instead. No documented control address is a name that does not resolve, and
  `docs/onboarding.md` no longer suggests `https://stado.wisent.com` as the
  value of `STADO_API_URL`.
- Documented in the CLI reference: `--offline` on `stado fleet invite`, both
  modes' stored objects and `--json` payloads, `open (offline, awaiting address)`
  in `stado fleet invites`, and a new
  [control-point check](docs/cli.md#the-control-point-check) section naming the
  verdicts apart — `not_configured`, `name_does_not_resolve`,
  `connection_refused`, `route_unknown`, `forced_offline` — each of which falls
  back to the fragment instead of printing a `curl` line that cannot work. The
  three invitation routes are now marked as useful only where the dashboard is
  reachable from the machine being added, which a loopback bind is not.
- [`docs/examples/fleet/invite-a-machine.sh`](docs/examples/fleet/invite-a-machine.sh)
  now runs the offline mode end to end — mint, fragment, the address back, then
  `fleet enroll --ssh … --bootstrap`, key check, grants, host recover and
  `registry beacon-age` as the proof — and keeps the one-line mode at the bottom
  with its prerequisites.
- Gave `stado fleet invite` the offline mode and stopped it printing a one-liner
  for a control point that cannot serve it. Before minting anything it asks the
  configured address — `STADO_API_URL` / `api.url`, never a name compiled into
  the binary — for `/join.sh`, and names the three failures apart: a host that
  resolves to no address, nothing answering the connection, and a live server
  that answers something other than 200, which is a release older than the
  invitation routes. Any of them, or no configured address at all, switches to
  the offline mode and says why; `--offline` chooses it without probing. An
  offline invite mints no secret and needs no route: it prints a self-contained
  POSIX `sh` fragment carrying the fleet's public key inline, which creates
  `~/.ssh` at 700 and `authorized_keys` at 600, installs that key idempotently,
  diagnoses a missing SSH server and prints the exact macOS or Linux step
  instead of enabling anything, and prints the `user@address` to send back —
  chosen by `deploy/join.sh`'s rules, in its order. The stored object records
  `mode` and, offline, carries no `secret_sha256` at all, so `authorize` and the
  dashboard routes have nothing a token could ever match; `stado fleet invites`
  shows such a row as `open (offline, awaiting address)`, and
  `stado fleet enroll NAME --ssh ADDRESS --bootstrap` closes it through the same
  `mark_spent` transition `fleet approve` uses for a redeemed token.
- Served the `invite` method from the dashboard: `GET /api/fleet/invite/key`
  hands the joining machine the fleet's public half and the exact
  `authorized_keys` line, `POST /api/fleet/join` files its pending enrollment
  request and spends one use of the invitation, and `GET /join.sh` serves the
  repository's bootstrap script verbatim and uncached. The two API routes are
  authorized by the invitation token alone — never by operator credentials,
  and not by the implicit trust a loopback caller has on operator routes — and
  write nothing outside `enrollments/`. Unknown, wrong, spent, revoked,
  expired and rate-limited codes all answer with one status, one sentence and
  the same elapsed time, so the routes cannot be used to enumerate or classify
  invitations; requests are bounded per code, per address and in size before
  any credential store or object store is read.
- Added the `invite` method to the fleet CLI, so adding somebody else's machine
  no longer requires reaching it first: `stado fleet invite [--name NAME]
  [--expires 24h] [--uses 1]` mints the fleet's own ed25519 channel key through
  the existing `fleet key generate` path and prints, once, the single line the
  machine's owner runs. The token is `<id>.<secret>` with 32 bytes of system
  randomness; the store keeps only `secret_sha256`, so nothing — no command, no
  stored object, no log — can reproduce a code after it is shown. `stado fleet
  invites` reports the state each invitation is actually in (`open`, `spent`,
  `revoked`, `expired`, the last derived from the deadline rather than waiting
  for a writer to notice), and `stado fleet revoke-invite ID` retires one.
  Expired, spent, revoked and unknown codes are refused identically.
- `stado fleet methods` (and `--json`) lists the four ways a machine can be
  added — `invite`, `adopt`, `join`, `declare` — with what each requires, what
  it provides, which registry field gates it, and whether this fleet's catalog
  allows it. It is the one source the CLI, Stado Desktop and the documentation
  read, so a method that exists is a method an operator can find.
- `stado fleet approve` now completes an invitation the same way the operator's
  own `enroll` does: it takes the SSH destination from the machine's request and
  runs the ordinary probing path — `hostname` and `uname` verified before
  anything is written, the entry rolled back if the agent will not install — and
  then marks the invitation spent. The machine is registered under the name its
  invitation reserved (which is the name its channel key was minted under), with
  its probed hostname recorded beside it. `stado fleet pending` gained `--json`
  and now shows that reserved name, the channel approval will dial, the
  invitation behind the request, and the key fingerprint the machine reports
  having installed.
- The enrollment catalog gained `allow_invite` and `allow_adopt`, reported by
  `stado fleet catalog` and honored by both methods' preflights. Like the
  existing allowances they default to permitted, including in an `enrollment`
  section written before the method existed.
- `authorized_keys` lines no longer name the credential item twice: the stored
  public key already carries `ssh-keygen`'s comment, so `stado fleet key
  install` was appending it beside a second copy.

### Coding clients

- Added `stado host jeden-connect` to place interactive Jeden RPC sessions on
  live registry hosts, require existing ledgers for resume placement, and carry
  the canonical bidirectional stream to native desktop clients.

### Service routing

- Directory consumer mutations now advance the routing generation atomically,
  preventing resolvers from rejecting changed directories as stale.
- Resolver adapters now close idle client streams after a bounded interval,
  preventing retained HTTP keep-alives from exhausting file descriptors and
  blocking every routed service.

### Local inference

- The documented `chat-primary` profile now uses a Featherless route for the
  same Cydonia model as its ordered fallback. With `gpu_mode=yieldable`, queued
  GPU work pauses local vLLM while Brama keeps chat available remotely.
- Route publication now accepts a temporarily stopped `yieldable` local primary
  when an ordered fallback is present; unavailable exclusive primaries and local
  fallbacks remain rejected.

### Credential recovery

- `stado credentials harvest --restore` now writes an owner-local Skarbiec vault
  through the Skarbiec CLI's field-aware contract instead of the retired
  whole-item HTTP payload. Restored values still move only over stdin and are
  never printed.
- Minting an SSH host key now ends with a key that can actually be read. Skarbiec
  authorizes reads per item, so a freshly written key was readable by nobody: the
  consumer every host channel authenticates as gained no capability from the
  write, and every new key was dead until an operator widened that grant by hand.
  `key generate` and `key add` now widen it themselves — preserving the
  consumer's bearer, its remaining lifetime, and every capability it already
  held — and prove the result by reading the item back through the same consumer
  the channel uses. A read-back that returns a different value still fails the
  mint: it means the broker serves a vault this machine's write never reached.
- `stado fleet enroll NAME --ssh DEST --install-key` adopts a machine that is
  not in the fleet yet. Enrolling presupposed that the fleet's public key was
  already in the machine's `authorized_keys`, because both the identity probe
  and `fleet key install` open the channel with the vault key itself — so
  adding someone's laptop began with an operator dictating a key over the
  phone. The flag installs it over a session the operator can already open
  otherwise: a loaded or forwarded ssh agent, one of their own keys, or
  OpenSSH's own password prompt, which OpenSSH asks and answers on its own tty.
  Stado never sees a password, the private half never leaves the operator's
  vault, and the line travels on stdin rather than in argv. A pair is minted
  through the existing `key generate` if the target has none, the append is
  skipped when the exact line is already present, and the run then continues
  down the unchanged path — probe the hostname and platform before the registry
  write, roll the entry back on a failed `--bootstrap`. The three ways first
  contact can fail now read as three different sentences, because they need
  three different actions: no connection was established, the connection was
  established and the credential rejected, or the credential worked and the
  machine's home directory refused the write. `registry.enrollment.allow_adopt`
  gates it.

### Core behavior

- The disk cleaner has a third cleaner, `build_caches`, so the automatic pass
  can reclaim build output. It knew only `huggingface_cache` and
  `weles_recordings`, which is why an operator laptop reached 8.8 GB free of
  1.8 TB — roughly 450 GB of build and scratch trees — while `disk-cleanup`
  had nothing to report. A directory is removed only when it carries a
  `CACHEDIR.TAG` whose first line is the Cache Directory Tagging Standard
  signature, the same criterion `stado host build-caches` already applied on
  request; no directory names or extensions are matched. Its policy takes
  `min_age_seconds` (at least 86400) and an optional `root`, defaulting to the
  host's `$HOME`, and it reports under `build_caches` like the other two.

### Service delivery

- `stado service converge` now also reports, per unit, `running_binary` and
  `binary_matches_process`: the executable the host's process table says the
  live process is running, and whether that is the artefact the unit's
  declaration resolves to today with neither file written since the process
  started. Every other column that command prints is about what is INSTALLED,
  and two incidents sat in that gap with all of them correct — Brama's process
  kept running an artefact tree `current` no longer pointed at, and the Weles
  worker kept serving a `dist` replaced 26 seconds after it started. The facts
  come from one extra read-only round trip per declared unit (the unit file,
  `readlink` on its `current` link, the process table, and `stat` on both
  files); the verdict is computed in the CLI so there is one opinion about
  artefact identity. A unit nothing runs under, or a host that would not say
  when a file was written, reports `null` in both and `unknown` in the new
  `PROCESS` column — never `true`, and never `false` either. A row may read
  `in-sync` and `differs` at once, which is the point.
- `stado service list --unowned` names the product processes on every
  `kind=local` host that no launchd job or systemd unit owns, with pid, command,
  start time and the product they belong to. Two `stado agent` processes ran on
  the always-on mac for four days with no unit behind them, executing a binary
  older than the one on disk, and every answer in this group was about declared
  units, so nothing surfaced them. A process counts when the executable it runs,
  or the entry point an interpreter was handed, lives under a managed root — the
  install root of every product in `stado-rs/data/products.json` plus
  `~/.stado/services` — so a `tail` on a log under such a root is not reported.
  Ownership is asked of the init system rather than assumed: the pids in each
  printable launchd domain's `services` table and their descendants on macOS,
  the `.service`-versus-`.scope` cgroup on Linux. It is the one read in this
  group the beacons cannot answer, so it costs one read-only ssh per host; it
  starts, stops and signals nothing, and a host that will not answer is named
  with a non-zero exit instead of being dropped from the list.
- `stado service ensure NAME --host HOST --from PATH [--arg A]... --reason WHY`
  asserts the unit a host must be running, idempotently and over ssh. `deploy`
  refuses a unit that is already declared and bootstraps into the per-user
  launchd domain, which does not exist on an ssh login: it answered
  `Could not switch to audit session ... Operation not permitted` and installed
  nothing, which is how the two unowned agents above came to exist. `ensure`
  reads what is there first and reports `already_correct` with nothing touched
  when the unit declares this exact argument vector and a live process under it
  runs that program, `restarted` when an existing unit was kicked in place, and
  `created` when there was no unit. Where the per-login domain is absent the same
  job is rendered as a launchd daemon in `/Library/LaunchDaemons` with a
  `UserName` naming the account it must run as, so the fleet's control binary is
  not run as root; a host without passwordless sudo is told that rather than left
  with a plist nobody loaded. An existing unit is only ever restarted in place
  (`kickstart -k`), never unloaded and bootstrapped back — that sequence took the
  always-on host down once — and a loaded unit whose definition names a different
  argument vector is refused rather than overwritten, because launchd holds the
  definition it bootstrapped. There is deliberately no fallback to
  `launchctl submit` or to a bare background process. The unit is recorded in the
  registry through the same validated write path `adopt` uses, `--reason` is
  required and refused blank, and any pass that changed something appends a
  create-only audit object beside the canonical registry document at
  `service_audit/<host>/<UTC>-<label>.json`.
- `stado service converge TARGET [BINARY]` reports the version
  `targets[].managed_versions` declares for each managed binary on that host
  against the version the host actually runs. Nothing could answer that
  question per host before: a service declaration named a unit and a plist
  path, both of which stay true across a release that never reached the box,
  so a mac mini serving an old build was indistinguishable from one at the
  declared version and `service list` reported `active` throughout. The
  comparison is a version and not a commit because that is the primitive these
  hosts carry — control-host runs Weles as an installed release artefact
  with `package.json`, `.weles-release` and `provenance.json` beside it and no
  checkout anywhere — and it is the same declaration `host inventory` and
  `host release` already judge against. The installed version comes from the
  read-only `report-installed-versions` helper over the existing
  `host run-helper` channel, which reads each artefact's own metadata and asks
  owner-only Stado programs for their version directly; a host that does not
  carry the helper reports `unknown` — never `drifted` — with the exact
  `host install-helper` command that fixes that. A product whose artefact
  carries no version metadata also reports `unknown`, named on stderr, and is
  never treated as in sync. Reporting exits non-zero on drift alone, so an
  uninstalled reporter cannot masquerade as drift.
- `stado service converge --apply` closes the gap by calling
  `stado host release --binary NAME --version X.Y.Z TARGET` in-process for
  every drifted binary, then re-reads the installed versions and exits non-zero
  unless every binary in scope is confirmed at its declared version, printing
  declared and installed side by side. There is deliberately no second delivery
  mechanism: the digest check against the canonical release manifest, the
  versioned staging tree, the `rename(2)` activation and the unit restart
  happen once in this pack. A declared binary `host release` does not carry is
  reported as undeliverable rather than as a failed delivery, and converge
  never writes the registry — the declared version is the operator's statement
  of intent, published with `stado host declare-version`.
- `stado host release` delivers any product the fleet declares, not the two
  binaries it used to carry in a compile-time table. `stado service converge`
  already read `weles-worker 0.5.1` off the registry and `0.5.0` off
  `/Users/charles/weles` and called the drift, and `host release --binary
  weles-worker` answered `"weles-worker" is not a stado-managed binary` — a
  drift report with no way to close it. A deliverable product is now a
  declaration in `stado-rs/data/products.json`, shipped inside the binary that
  performs the delivery, naming the artefact source and archive member, the
  platform keys it is published for, the install root on the host, the owning
  unit label where one exists, and how the installed version is read back.
  `stado`, `skarbiec` and `weles-worker` are three entries with equal standing;
  no hardcoded product list remains, and `host inventory` reads the same
  declaration instead of spelling `stado skarbiec` into its remote program.
  Every field is required: a declaration that omits one, points an install root
  outside `$HOME`, claims an unpublished platform, or reads a tree's version
  out of a path a delivery must preserve is refused when the declaration is
  first read.
- A declared product may install a tree rather than a single program.
  `weles-worker`'s install root is the artefact directory itself, so a delivery
  unpacks the declared payload into the versioned staging tree, verifies the
  version the staged tree declares, then replaces the code path by path, one
  rename each, retiring what it replaces — and leaves the declared host-local
  paths (`recordings`, `var`, `.work`) exactly where they are. They are never
  named as a destination, never moved, and an artefact that carries one of them
  is refused both at staging and again at activation. Every check a program
  delivery makes still applies: the immutable manifest identity, the archive
  digest, the platform the host reports, and a version read back from the
  delivered artefact before the unit is restarted. `--dry-run` prints the
  artefact it would fetch, the checks it would run, the paths it would replace,
  the paths it would preserve and the unit it would restart, and sends the
  read-only probe and nothing else.
- Publication is per product, so `host release` and `host promote-version`
  refuse a platform the product does not declare instead of fetching a
  coordinate nobody published. A product declaring only a unit label still has
  to have that label found in the registry's declared service set before
  anything restarts; a product declaring the label with its unit file locates
  the unit itself, which is how the Weles worker's LaunchAgent is addressable
  without a registry record for it.

## 0.5.0-rc.1 - 2026-07-29

### Product contract

- Reframed Stado around the supported 0.5 product boundary, intended users,
  explicit non-goals, and capability-status semantics.
- Froze stable 0.5 support to local execution, local filesystem storage, and
  their provider-neutral queue, recovery, artifact, API, dashboard, MCP, and
  scoped-secret contracts.
- Declared cloud storage, cloud VM, Box, and Vast adapters preview until each
  integration has release-scoped live acceptance evidence.

### Release engineering

- Replaced the split tag-triggered publication path with one default-branch
  release/delivery run using standard `v<version>` tags.
- Unified crate licensing with the repository Apache License file.
- Defined nightly, candidate, and stable channels, immutable release manifests,
  supported platforms, compatibility rules, and upgrade/rollback gates.

### Onboarding

- Added a no-argument first-run path and a minimal local configuration contract.
- Removed cloud credentials, product-specific API clients, and optional
  integrations from the required local onboarding path.
- Documented enrollment as the verified path it is: `stado_fleet key generate`
  prints the public key that first contact needs, `stado_fleet enroll` probes the
  machine's hostname and platform before writing them, and `stado registry host
  add` is the declaration on its own. The documented `host add` invocations were
  missing the required `--release-platform` and failed as written.
- Gave `stado_fleet` a build and install path of its own. Having none is how this
  control plane came to run `stado_fleet` 0.5.1 against `stado` 0.7.2 from one
  shared library, until `stado_fleet key ls` began answering HTTP 400 against the
  current Skarbiec field-read contract. `install-built-stado-binary.py` now also
  accepts a repair: where the running binary fails a read-only probe the
  candidate passes, agreement with the broken binary is not required.
- Made enrollment part of `stado`: adding a machine is `stado fleet enroll`, with
  `join`/`pending`/`approve`/`reject`, `key generate|install|check|rotate|ls|rm|add`,
  `list`, `status`, `create`, `assign`, `catalog` and `doctor` beside it. The
  dashboard's operator console can now run enrollment, which it never could: it
  executes `stado`, and enrollment existed only inside the separate `stado_fleet`
  binary, so the first command a new machine needs was absent from `stado --help`
  and from every surface built on it. `stado_fleet` keeps every command, flag and
  word of output, now as a thin entry point onto the same library code — there is
  one implementation, not two. Both `stado_fleet` and `stado_migrate` are also
  declared in the crate manifest instead of being found by directory: nothing
  naming them is what let `stado_fleet` run 0.5.1 against the 0.7.2 library it
  shares with `stado` for weeks, with no command able to report the gap.

### Credentials

- Restored every write into a Skarbiec-backed credential store. `PUT /v1/items`
  became the Weles acquisition route when the vault contracts were rebuilt, and
  it requires `id`, `field` and `operation_id` and refuses an item it does not
  control; Stado still sent whole items, so `stado credentials put`,
  `stado_fleet key generate|add|rotate` and the Azure operator credential all
  answered `400 {"error":"field required"}`. The fleet could read credentials and
  could not mint one, so no new host could be enrolled. Writes and deletes now go
  through the vault's owner, in one place inside `credential_store`, instead of
  one command knowing the contract and the rest guessing.
- Named Skarbiec's canonical kinds and its field/context split where Stado writes
  them: a host key is a `key-pair` with the two halves as fields and its
  fingerprint and key type as context, and the Azure operator session is a
  `stado-secret` rather than an `oauth-client` that allows only two fields.
  `stado_fleet key ls` reads that context instead of printing two blank columns,
  and `key generate` reads the new item back through the same client the SSH
  channel uses — an owner write reaches a vault file while every consumer reaches
  a broker, and on a host whose broker forwards to another machine's vault those
  are different stores.

### Core behavior

- Added durable cancelled records and canonical lifecycle reconciliation.
- Replaced the global Python preflight and CUDA probe with runtime-scoped,
  native checks; optional Hugging Face cleanup remains isolated.
- Hardened the agent loop, public machine contract, results/artifact handling,
  secret redaction, pause/drain recovery, and versioned config/storage schemas.

### Integrations

- Stabilized the shared storage contract for local filesystem, GCS, S3, and
  Azure Blob, including conditional writes, listing, metadata, and recovery.
- Kept GCS, S3, Azure Blob, GCE, EC2, Azure VM, Box, and Vast as preview.
  Current live attempts reached provider APIs but could not qualify them:
  GCP billing is disabled, the available AWS access key is invalid, and no
  Azure managed-identity sandbox is provisioned.

### Verification

- The current Rust tree passes 697 tests across all targets and features,
  with four provider-live suites ignored by default.
- Clippy passes with warnings denied and rustfmt reports no differences.
