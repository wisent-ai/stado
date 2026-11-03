# Changelog Unreleased (part 19)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Changed

- **A model request is handed to Stado, and Brama's alternative is a statement recorded for the
  request:** a product resolved `inference`, then dialled what it was handed itself, reading the
  endpoint's key and choosing whether to sign, so every alternative added to the catalog needed a
  change in every product. `stado capabilities call inference --consumer C [--json]` takes the
  OpenAI chat-completions request on standard input, asks the first alternative in the selected
  order that answers it and prints that answer; one that cannot answer this request is named with
  its cause and the next is asked. Brama is sent the request with the `token` of
  `<consumer>-model-router` and, when an item plays `<consumer>-agent-auth`, its signature. The
  OpenAI-compatible `model-endpoint` alternative and its `capabilities.inference.endpoint.*`
  settings are removed; `attestation` takes its place: a request no alternative before it
  answered is kept under its SHA-256 and refused naming the command that answers it, and
  `stado capabilities attestation list|show|record|revoke inference --consumer C` reads it and
  records the answer with what was checked, answering that exact request as a chat completion
  whose `model` is `attestation`. Stado Desktop runs the same operations on the Compute providers
  screen.

### Fixed

- **No check formats or lints a tree:** `stado quality check --pre-push`, `stado release changes
  submit` and the delivery queue's gate step check the committed source alone (lock resolution,
  module declarations and reach) and run no gate; a push to `main` no longer runs `lint` gates on
  fleet hosts and asks Oko no admission, and `--host` on `--pre-push` is gone with it. The full
  `stado quality check`, release submission and `stado change run` run only `test` gates; a
  `format` gate runs only under `stado quality format`, which writes. Whether code compiles is
  learned from reading it and from the tests that compile it. A refused web gate is named as a
  web gate whatever its kind, and the source-only delivery check that ran beside the format
  gates is gone with them.
- **`stado product install` and `update` build without formatting, linting or checking first:** a
  source install ran every quality gate its platform declares (`cargo fmt --check`, `cargo clippy`,
  `stado errors check`) before the build step, recompiling a revision the push had already checked
  and holding the install for many minutes, and a recorded gate failure refused the revision before
  it was exported. An install now exports the committed revision, builds and places it; whether the
  code compiles is learned from its tests and from reading it, and a revision that does not build
  fails the build step naming its evidence. The `install-failures` records are no longer written
  or read, and the next install of a checkout removes the ones an earlier Stado kept in its build
  area. Stado's own manifest declares no `format` or `lint` gate any more (`fmt`, `clippy`,
  `error-standard`), so no push, queue pass or release of Stado runs them either; its test gate
  stays.
- **`stado release submit`'s CLI audit no longer runs a declared launcher:** the member a release
  manifest's `runtime.launcher` names (`bin/start`) is started by release control with the binary,
  port and runtime directory it assigns, so the audit no longer reads its `--help`. Run bare,
  Skarbiec's launcher exited 1 on `SKARBIEC_BIN` and refused the release.
- **`stado release submit` carries a published diff that is not UTF-8:** a committed binary file git
  reads as text (Skarbiec's compressed inflate test vectors) put bytes that are not UTF-8 into the
  diff the source archive records for its gates, and the submission was refused with `answered
  non-UTF-8`. The record now carries exactly the bytes `git diff` gave, as `diff` when they are
  UTF-8 and as `diff_base64` when they are not, and its `sha256` is the digest of those bytes, so
  it can be checked against `git diff` itself.
- **A placement move carries every state as a file, and copies it before the fence:** a file state
  travelled as base64 inside the read script's output and the write script's heredoc, held whole
  in the move's memory with the source fenced for the whole copy, so a state of a few gigabytes
  held its service down for hours and then failed with `bash: xrealloc: cannot allocate`.
  Every state, file or directory, now goes through `rsync -a` into a staging copy on the machine
  running the move and on to a stage beside its path on the destination, renamed over it after the
  previous file or tree is renamed to the backup. The preflight and a first copy of each state
  (`copy <state> while it serves`) run before the move claims its transaction, since an open
  transaction refuses the profile's services to every consumer; the claim is then laid on the
  registry as it stands and refused when the profile moved meanwhile. After the fence each state
  is read again by content (`read what changed in <state>`), so the services are refused for what
  changed in between; a role move copies its store the same way before it arms the store fence.
  The staged copies are removed once installed, when the move rolls back and when it fails before
  its claim, and the local staging room counts every state.
- **A Stado built from an exported tree no longer claims the enclosing checkout's revision:** the
  build asked `git rev-parse`, which walks up to any enclosing work tree, so a tree exported inside
  another checkout reported that checkout's HEAD with `-dirty`. Git is now asked only when the
  package's parent is the root of its own work tree; otherwise the build embeds the revision
  `STADO_SOURCE_REVISION` states, or `unknown`.
- **A move of the fleet vault can commit:** the move stopped the vault's unit as its fence, and
  the registry authority asks that vault for its store bearer's version on every write, so a move
  of the vault off the authority's host copied every state and then failed at `commit the
  placement` and rolled back. The profile carrying `skarbiec` now holds the vault's writes with
  `skarbiec fence arm --transaction <T>` (step `hold the vault's writes`), so every read keeps
  answering through the commit. After the commit the source host's resolver is asked to follow
  and the source's units are retired, then the retired copy's fence is lifted so a later move back
  finds no fence standing; each of those that fails is listed under
  `unretired_source` and does not roll back a committed move. A move that fails before the commit
  lifts the fence instead of restarting units that never stopped. The preflight refuses a source
  whose Skarbiec has no write fence, a source whose `skarbiec` unit does not run that Skarbiec or
  whose own `registry doctor` reports the unit's process as not executing its installed file (an
  old process ignores the fence on its writes), and a source whose own Stado reads the vault at
  the unit the move retires, naming the configuration that sends it through its resolver adapter
  instead.
- **The fleet vault keeps resolving while its own move is open:** the resolver refused every
  service of a profile under an open placement transaction, so once a host's registry authority
  and object API read the vault through their resolver adapter (`secrets.skarbiec.url
  stado://service/skarbiec?consumer=stado`, what the vault move's preflight asks for), the move's
  own claim cut them off from the vault every store write is checked against: the claim landed,
  its receipt never came back (`received unexpected message from connection` from the adapter's
  502), and the transaction stayed open with the vault refused to everyone. `skarbiec` now
  resolves to its active host, the write-fenced source, until the commit retargets it; every
  other service of a moving profile is refused as before.
- **A move whose commit landed is not rolled back when its receipt is lost:** the registry
  authority writes the commit and then answers, and its answer can fail after the write (a vault
  move's authority loses the vault for a moment once its resolver follows the commit). The move
  took that as a failed commit and rolled every host back under a registry that already placed
  the profile on the destination, so every consumer was sent to a host that no longer served it.
  A failed `commit the placement` now reads the registry back: this move's transaction released
  and its services active on the destination is a commit, and the move goes on as committed; a
  registry that does not hold it rolls back as before; one that cannot be read leaves the hosts
  as they are and says how to tell (`placement transactions`: the transaction still open means it
  did not land, and `placement release` undoes it).
- **The read-back of a lost commit asks the registry authority itself:** it read the caller's
  resolver copy, which had frozen at the move's claim while the authority's store was down, so a
  commit that had landed was taken for one that had not and the hosts were rolled back under it
  again. The read now goes to the authority over its own SSH paths, as the commit does; when the
  authority does not answer, the outcome is unknown and nothing is rolled back, and the error
  says to run `stado resolver follow` before `stado placement transactions`. A host that is the
  authority reads its store as before.
- **The vault move's refusal names the reload:** when the source's Stado reads the vault at the
  unit the move retires, the refusal named `stado host config set … secrets.skarbiec.url …` as if
  the value took effect alone. It now names `--reload-service stado`, run on the source, whose
  owner vault holds the host account a system daemon's reload needs.
- **A vault move commits only after its source has read the destination's vault:** the
  destination's probe asked `GET /health` on its own loopback, which reads nothing, so a move
  committed while the vault there answered nothing over the channel from the source; the
  source's registry authority then checked every store request's bearer against it and its
  store went down with the commit. A new step before the commit, `read the destination's vault
  from <source>`, runs `stado host local vault-reach` on the source: every object API bearer's
  revision read as consumer `stado` over the native SSH channel the source's resolver will open,
  within the resolver's refresh interval. A source that cannot is refused before the commit and
  the move rolls back, with each read's namespace, item, time and answer in the refusal.
- **A vault reach that runs out of time says where the time went:** a reach that exceeded the
  resolver's refresh interval reported only `reads: []`, so a channel that never opened could not
  be told from a vault that did not answer. `stado host local vault-reach` now reports `paths`
  (each SSH path tried, in order), `paths_seconds` (choosing them, which reads the tailnet's
  status), `session_seconds` (the SSH session opening) and, when the bound expires, a cause naming
  the phase it expired in: the path choice unfinished, no session over the named paths, or the
  read still unanswered with how many had answered.
  The receipt's `attempts` lists each SSH path tried, alone and in the order the resolver's
  tunnel tries them, with its seconds, whether it opened and its error; a bound that expires
  while a path is connecting names that path and every earlier path's failure.
- **A tailnet path is left out when this machine's own Tailscale carries no traffic:** a path was
  left out only when the peer's node key had expired, so a host whose own key had expired, or
  whose Tailscale was logged out or stopped, kept every tailnet path and its connection waited
  with no end, holding a resolver's tunnel and a vault move's reach for their whole bound. Such a
  path is now left out naming `this machine's own Tailscale node key expired at <T>` or `this
  machine's Tailscale is <state>, not Running`; when no other path is left the connection fails
  naming it, as for an expired peer. A Tailscale client that answers `status --json` with
  something other than JSON (one still starting, or asking for a login) is named with the first
  200 characters it printed on stdout and stderr, where the message gave only the parser's
  position before.
- **`--host` on a surface that installs locally is refused, not ignored:** `stado product install
  stado --surface cli --source-commit <c> --host <h>` built and installed on the machine running
  it, reported `installed`, and left `<h>` as it was, so the caller believed `<h>` ran the new
  build. `install` and `update` of a `cli` or `desktop` surface given `--host`, and `sync` of one,
  are now refused before anything is built, naming that `--host` would be ignored, how to reach
  the named host (run the installation there, or deliver a release with `stado release prepare`
  and `stado release promote`) and that nothing was built or installed. The narrower refusal of a
  CLI release coordinate with `--host` is folded into it. Stado Desktop's Products screen passes
  the selected host only for a `service` surface, as it already runs every other surface on the
  machine it sits on, and its sync operations label the host field as one for service surfaces.
  Real journey: `tests/install/host_surface.mjs`.
- **A vault move authorizes its source's resolver key on the destination:** a source that is the
  registry authority never opened a session to another host, so it held no resolver SSH key; the
  vault reach, and the resolver once the move would commit, failed with `SSH authentication
  failed for <user>: no identity is available`, and the destination's `.local` path with an
  untrusted host key, while the vault there was up. `stado route key TARGET --to HOST`
  authorizes TARGET's resolver key (minted on TARGET when absent) on any registry host its
  adapters tunnel to, not only on the directory authority, and the vault move runs it as the step
  `authorize <source>'s resolver key on <destination>` before the reach. The receipt names the
  host as `authorized_on`; `--to` naming TARGET itself, and the authority without `--to`, are
  refused with their cause. Stado Desktop's **Authorize this host's resolver public key** takes the
  host. Real journey: `tests/route/key_to.mjs`.
- **A burst of refused LAN connections reads macOS's log once, not once per connection:** every
  connection to a LAN peer refused `No route to host` read what `nehelper` decided about Local
  Network for the running binary with a `log show` over the whole log since the binary was placed.
  A network node that could not reach its peers ran dozens of those scans at once over days of
  log, holding the host's load in the hundreds and slowing every connection through it, the
  vault move's reach among them. One read now runs at a time in a process; callers that ask
  together wait for it and take its answer, and each later read covers only what was logged since
  the previous one began, keeping the earlier decision when nothing newer was logged.
- **A vault move's reach bounds each read, not all reads together:** `read the destination's
  vault from <source>` gave the session and every object API bearer read one refresh interval in
  total, so with nineteen namespaces declared a channel whose reads each answered in a fraction of
  a second was refused at the tenth read, though the object API makes one read per request and
  the resolver gives each request up alone. The session must still open within the interval;
  each read now has the interval less the session's time, the request that opens the session.
  The receipt names it as `read_bound_seconds`, and a read past it names how long it was waited
  for. Real journey: `tests/placement/vault_reach.mjs`.
- **`placement move --json` prints only its report on stdout:** a move printed its progress
  (`moving <profile>: <from> -> <to>`, each step, each probe) on stdout before the JSON report, so
  `credentials vault move`, which runs `placement move --json` and reads that report, refused a
  move that had committed with `printed no readable report`, and its error hid why. Progress now
  goes to stderr; a report that still cannot be read is quoted with the start of stdout and the
  end of stderr, and the refusal names `stado placement history` and `stado placement holds` as
  where to read whether the move committed.
- **`credentials vault move` finishes a move whose placement committed:** a move whose
  placement committed while its caller could not read the report left the directory placing the
  vault on the destination, the destination declaring none and the former host still declaring
  its old file, and running the move again refused `<host> already holds the fleet vault`. A move
  to the host the directory places the vault on now reads that host's owner and every host's
  declaration and, with `--apply`, declares the vault there and withdraws it everywhere else,
  then requires that host's `keys check` to read it. Stado Desktop's **Move the fleet vault and
  its owner to another host** runs the same command.
- **`placement standby` advances the directory generation when it writes a standby address:** the
  standby commit added the new host under the service's `standby` in the service directory
  without advancing its generation, so every resolver refused the store (`service directory
  changed without advancing generation <N>`) and kept the directory before it; the write landed
  and reached no host. The commit now advances the generation whenever it writes the directory,
  and every programmatic registry write that changes the directory under the generation the
  registry already carries is refused before the store sees it (`this registry write changes the
  service directory but leaves its generation at <N>; … nothing was written`), as uploads were.
- **A wait for the resolver to serve ends when it publishes that its registry read failed:** `service
  ensure stado`, run by `product install`, waited for the restarted resolver to publish `serving`
  and took only `failed` as the other verdict, so a resolver that kept publishing `backing_off`
  (its store refused, here a directory changed under an unadvanced generation) held the install
  for an hour until it was killed. `backing_off` now ends the wait with the resolver's attempt,
  next read and reason.
- **A placement move rewrites the source account's home in the files it carries:** a home-rooted
  file state was copied verbatim between accounts, so Weles' `worker.env` moved from one host to
  another still named `STADO_BIN=<source home>/.stado/bin/stado`, and the moved `weles-admission`
  exited on start with `Required Stado binary is unavailable: <source home>/.stado/bin/stado`. A
  home-rooted file that is UTF-8 now has each mention of the source home (followed by `/`, a quote
  or the line's end) rewritten to the destination's home in the copy before it is sent; the move
  prints how many it rewrote. A directory state and a file that is not UTF-8 are carried as they
  are.
- **A move's health probe takes a 2xx only from the moved unit's own process:** a placement move
  read the destination's probe URL and committed on the first 2xx, so another program listening on
  the port the directory gives the service (here a different product on Weles' port) answered for a
  `weles-admission` unit that had exited, and the move committed a service that never ran. A 2xx
  or a 4xx now counts only once the socket table shows the unit's process listening on the URL's
  port; a unit that stops running while another program answers there is refused naming that
  the port is taken.
- **`placement standby` refuses a port another program already listens on:** standby staged a
  unit on the port the service directory gives it on the new host without asking whether anything
  listened there, so Weles was staged on a laptop port another product already held, could not
  bind it after the move, and exited. Before it stages each unit, standby reads the host's socket
  table for that port and refuses naming the listener's pid, command and user and the host's own
  words, so the directory's standby address is given a free port first.
- **A unit is never written with `$STADO_LISTEN_PORT` left in it:** a unit rendered while the
  service directory recorded no port for the service on that host kept the word as its text, and
  launchd and systemd expand no variables, so Weles staged on the laptop started with
  `WELES_API_PORT=$STADO_LISTEN_PORT`. The word is replaced in a host's own unit declaration as in
  the catalog's, and a unit that would still carry it is refused naming where it stands and how to
  record the port.
- **A vault declared as `$HOME/<path>` is read under the account's home:** `credentials vault
  move` declares the moved vault on its new host as `$HOME/.stado/skarbiec.vault.json`, and that
  host's own credential verbs read the declaration literally, so `credentials vault show` answered
  `state: declared-absent … $HOME/.stado/skarbiec.vault.json is not a file on this machine` beside
  a vault that served. A declaration under `$HOME/` (as under `~/`) is now read under the
  account's home by every reader of it.
- **`release submit --commit` looks for each step's program in that commit:** the check that every
  release step's script exists read the checkout's working tree, so a checkout standing on another
  commit refused a submit of a commit that carried its scripts (`skarbiec: darwin-arm64 step
  serve-one-process runs tests/release/journey.sh … nothing was enrolled`). A submit now checks the
  submitted commit's tree and names a missing program as `<path>, which commit <commit> does not
  hold`; enrolling a checkout keeps checking its working tree.
- **A service surface built from source is placed where its catalog program runs:** `product install
  <product> --surface service` placed a source build as a CLI is, each `bin/` member into the shared
  `~/.stado/bin`, so Weles' `bin/start` launcher was refused as `~/.stado/bin/start belongs to another
  recorded product` (Skarbiec's and Brama's launcher of the same name), and the catalog program
  `$HOME/.stado/services/<service>/current/<platform>/<launcher>` was never written by the install.
  A service build is placed whole under that directory, each staged member at its staged path, with
  no link on PATH; a release that stages no file named as the catalog program, or a catalog program
  outside `~/.stado/services`, is refused before anything is placed. Real journey:
  `tests/install/service_tree.mjs`.
- **`host run attach` returns when its program exits, even if a service it started holds the
  output:** the attached program wrote straight into the SSH session, so `release local install` on
  a host, which restarts `stado serve`, left that service holding the session's stdout; the attach
  waited until its caller killed it and ended with `attached process signal marker is
  unavailable`. The program now writes into two relays the wrapper owns: once it exits, the
  wrapper names each process still holding a relay (pid and command), stops relaying and exits
  with the program's status. A signal forwarded after the program ended names the marker and
  why it is gone. Real journey: `tests/host_run/attach_descendant.mjs`.
- **The host-health loop reads launchd's log over its own period, not an hour:** the first read of
  each unit and the beacon's interface-change window both took their span from
  `WC_HEALTH_INTERVAL_SECONDS`, which `stado serve --health-interval-seconds` never sets, so every
  first read covered an hour (one `log show` per unit, minutes each on a loaded Mac, holding the
  beacon 40 minutes) and every beacon published its interface changes as unread. Both now take the
  serve loop's period; a one-off `beacon collect` reads the hour and says it has no window. A
  collection that outruns its period is logged with its duration.
- **`service env check` and `service env show --env-file` read an env file on a Mac:** a comment
  inside the single-quoted awk program held an apostrophe (`a key's name`), which ended the
  program's word and left the rest of the script unbalanced, so every read ended `unexpected EOF
  while looking for matching`; the last line also sanitized the path inside a double-quoted
  `"$(… awk '…["\\]…')"`. The comment holds no apostrophe and the path is sanitized in a plain
  assignment. Real journey: `tests/service_env/check.mjs`.
- **A sync's finish no longer stops the operator's own install:** `product sync` waits for an
  installation under way and never supersedes it, but the `product finish` it hands its built Stado
  to was started without `--wait`, so it took the surface lock by superseding: an operator's
  `product install stado --wait` that had taken the lock in between was sent SIGTERM mid-build and
  ended 143 with nothing said. The finish now takes the lock the way its installation did.
- **`stado placement rehome PROFILE --host HOST --from-home DIR`:** a move made before the home
  rewrite left every home-rooted file it carried naming the source account's home (Weles'
  `worker.env` named `STADO_BIN=<source home>/.stado/bin/stado` on the destination, byte for byte
  the source's copy). This rewrites DIR to HOST's own home in each such file state of the profile
  on the host it runs on, through the read, rewrite and staged install a move uses, keeping the
  previous file as `<path>.replaced-by-placement-<id>`. Directories, work-rooted states and files
  naming no DIR are reported and left as they are; a profile that runs elsewhere, an unknown
  profile and a relative DIR are refused.
- **The Weles unit is rendered with its host's recordings directory:** the Weles launcher refuses
  to start without `WELES_RECORDINGS_ROOT`, which only the `weles-recordings` workload wrote into
  the plists already on a host, so a unit `placement standby`, a move or `service ensure` rendered
  for a new host started without it and exited at once. The catalog's Weles unit now takes it from
  the host's `weles.recordings_dir` (`${declared:weles.recordings_dir}`); a `${declared:<field>}`
  placeholder reads a dotted path into the registry entry, and a host that declares none is
  refused naming the field and how to declare it.
- **launchd's event log is read beside the beacon, never before it:** the period fix still had the
  serve loop read each unit's launchd events before it published, and on a loaded Mac the first
  `log show` of one unit ran past seven minutes, so the beacon stood still that long. The beacon
  now reads each unit's state alone and carries the events of the last completed read with its
  age (`events: ok (<n>s ago, read beside the beacon)`, or `not read yet` before the first); the
  event reads run in one background pass per period, a pass still running is not started again,
  and a pass longer than the period is logged.
- **`host run attach` names a relay's holders on macOS:** the wrapper asked `lsof -t <fifo>`, which
  on macOS names no process holding a FIFO, so a program's lingering child went unseen and the
  attach still waited for it (93 s in `tests/host_run/attach_descendant.mjs`). Holders are now
  found by the relays' device and inode among every open file.
- **Stado keeps the fleet signing identity in its own keychain:** Skarbiec's recipes declared the
  operator's login keychain, which holds his own identity and not the fleet role's the installed
  Skarbiec is signed with, so every source install on a Mac was refused as an identity change and
  could proceed only by changing the identity by hand. A recipe now declares `signing_role` beside
  `signing_keychain`; at every installation Stado reads the role, imports its identity into the
  declared keychain under `~/.stado/signing` when it is missing or renewed, and signs from it. A
  role that cannot be read (Skarbiec down for its own update) signs from the identity kept
  earlier; with none kept yet the install is refused naming the role. The receipt records
  `signing.kept_identity` (role, `kept`/`imported`/`unrefreshed`, SHA-1); Products in Stado
  Desktop says which role is kept. Skarbiec's three recipes declare
  `$HOME/.stado/signing/fleet-identity.keychain-db` and `macos-development-signing`.
- **An explicit installation no longer waits behind an automatic sync's build:** `product install
  --wait` waited for whatever held the surface's lock, so on a loaded Mac each pass of the host's
  `product sync` building origin/main (half an hour of Stado's own build) held every explicit
  installation of the same surface until it ended. An explicit installation now supersedes a sync
  that is still preparing, as an installation without `--wait` does, saying `superseding the
  automatic product sync …`; a sync that is placing files, and every other holder, is still
  waited for. The sync installs again on its next pass.
- **A host past its declared swap ceiling starts no new work:** a Mac with its swap 97% used kept
  claiming queue work, launching routines and starting product builds, and every one only paged: a
  Stado build took over half an hour with its compiler at two percent of a core. A host may declare
  `host.swap_ceiling_pct` (`WC_HOST_SWAP_CEILING_PCT`). Above it the worker publishes
  `swap_ceiling_exceeded` and claims nothing (`diag.swap_ceiling` carries the reading), the host's
  disk scans are skipped as over the load ceiling, and `product install|update|cargo|swift` wait
  before exporting or compiling as a `czekam:` host wait naming the reading, starting once swap
  falls under the ceiling. Nothing is assumed for a host that declares none; an unreadable swap
  admits the work and says why.
- **The host process no longer keeps a zombie per job it held the Mac awake for:** each job a
  worker started on a Mac took an idle-sleep assertion (`caffeinate -i -w <job pid>`) whose handle
  was dropped, so every assertion that ended with its job stayed a zombie child of `stado serve`
  (44 within an hour of Oko's routines on one laptop). Each assertion's status is now collected
  when it ends; a thread that cannot be started to collect it is logged.
- **The service reconciler reads a host's units without the full inventory:** each pass of the
  coordinator's predecessor reconciliation ran the full loaded-unit inventory, which greps every
  fleet script, forks `ps`, `date` and `stat` per loaded label and walks the host's PATH, though
  it reads only labels, domains and declared and running programs. On a loaded Mac that read ran
  for over eleven minutes inside `stado serve` each pass, adding to the host's load. It now takes
  the image read `service images` already uses for the same fields.
- **The beacon's link block is collected beside the beacon too:** after the launchd event reads
  moved beside the beacon, the link block's own reads (`log show` over configd for interface
  changes, `pmset -g log`, the tailnet path) still ran before each publish, and on a loaded Mac the
  laptop's beacon reached 193 s against a 60 s period. A beacon published on a period now carries
  the newest link block collected beside it (its `collected_at` says when); before the first one
  finishes the block's `source` is `pending` and `unread` says the collection runs beside it. A
  one-off `beacon collect` still collects its own block.
- **`host run attach` matches relay holders among FIFOs by inode:** the device-and-inode match
  found nothing on macOS, because lsof prints no device field for a FIFO, so the attach still
  waited 90 s for a program's lingering child. Holders are matched by inode among records of type
  FIFO.
- **A signing scope removes the identity material killed scopes left:** a scope that was ended
  before its cleanup ran left `~/.stado/signing/.wisent-identity-<uuid>/` on disk with the
  signing role's private key in plain PEM and a keychain holding it (two were found on one Mac).
  Only one scope runs on a host at a time, so each new scope, once it holds the host's signing
  lock, takes every such keychain off the search list, deletes it, removes the directory and says
  so; a directory that cannot be removed refuses the signature naming it.
- **The kept role keychain is on the search list when `codesign` signs:** the first Skarbiec
  install that kept the fleet identity imported it into `fleet-identity.keychain-db`, which
  `security find-identity` then listed as valid, and `codesign --keychain` still answered `no
  identity found`: the keychain was not on the user's search list, which `codesign` builds the
  identity's chain from. Stado now unlocks it and puts it first on the list when it keeps the
  identity and again right before each signature, as it does its temporary keychains.
- **A superseded surface of the same recipe no longer holds its sibling's files:** Skarbiec's
  `cli` install built, signed and verified its binary, then was refused with `installation would
  change ~/.stado/bin/skarbiec, which skarbiec/service (Superseded) still owns`: the service
  surface's interrupted installation had been superseded and never finished, and only `installed`
  or `rolled_back` siblings could be replaced. A `superseded` surface installed from the same
  kind, repository and manifest is now a sibling: the install replaces the shared files and
  re-records that receipt with the new fingerprints and revision.
- **A journey under an isolated HOME no longer opens keychain dialogs:** every git a journey's Stado
  ran found no `~/.gitconfig` under the isolated HOME, fell back to the system file's
  `credential.helper osxkeychain`, and asked macOS to store the GitHub token in a login keychain
  that HOME does not have: the person saw one "Keychain Not Found ... Reset To Defaults" dialog per
  fetch. Every journey that sets HOME now spreads `pinnedGit` from `tests/native/home.mjs`:
  `GIT_CONFIG_GLOBAL` names the real `~/.gitconfig` and `GIT_CONFIG_NOSYSTEM` drops the system
  file, so the person's `gh auth git-credential` helper answers and no keychain is asked.
- **Placement standby installs a `replace` product's tree itself:** standing a host by for Brama
  answered `awaiting_release_rollout` with `brama rollout_pending: … its agent stages the desired
  release`, and no pass ever did: the release agent drives only blue-green policies, and a
  `replace` release reaches a target only when it is promoted. Standby now installs a `replace`
  product's desired tree on the host through the same verified fetch and archive installer
  (`already_active` when that archive is the active one), then stages its unit and writes the host
  into the profile and the directory.
- **An installation reads only its committed catalog:** `product install`, `update` and `finish`
  first read and validated the checkout's working-tree `catalog/products.yml`, then read the
  committed one they execute. A checkout behind origin therefore refused an install whose own
  revision held a valid catalog: Stado's finish of a revision that names every product's
  `star_repository` was refused with `ster names no star_repository` because the checkout's file
  predated the field. The verbs that build now read the catalog only from the commit they install;
  `status`, `remove` and `rollback` read the working-tree file as before.
- **Stado finds `cargo` in `CARGO_HOME`:** a build or quality step's bare program was looked up in
  `~/.cargo/bin` only, though Cargo's home is `CARGO_HOME` when it is set. A Stado run under another
  HOME with `CARGO_HOME` naming the toolchain was refused with `no executable cargo in Stado's and
  the toolchains' homes`. The search now reads `$CARGO_HOME/bin`, and `~/.cargo/bin` only when
  `CARGO_HOME` is unset.
- **The submit queue's harsh review reads the candidate against its own parent:** `stado change
  run` ran Tama's review in the repository's shared checkout, whose HEAD is whatever the person
  last checked out, so the reviewer judged every commit between that HEAD and the candidate as the
  change, and Tama refused rounds over a tree staged on a newer parent. The review now runs in a
  bare repository of its own under the checkout's Git directory, borrowing its objects, with HEAD
  the candidate's parent and the candidate's tree in its index. The reviewer's repairs are
  committed there and fetched into the checkout before that repository is removed.
- **`stado product cargo` and `swift` find their product in the committed catalog:** to ask Oko
  about the build of the product a manifest belongs to, they read the checkout's working-tree
  `catalog/products.yml`, so a checkout older than a field the catalog now requires refused every
  Cargo build of every product with `ster names no star_repository`. They now read the catalog an
  installation reads: the checkout's committed head, or `origin/main` when the head is behind it.
- **No harsh review runs for a candidate whose author Oko refused to enter:** when Oko's
  harsh-review queue already held an open entry for the submitting session, `stado change run`
  ran the review anyway and Tama judged the tree as written by the session running the pass. The
  pass now runs no review then: the candidate leaves the head of the queue with `no-round`, naming
  the entry that holds its author. A machine without Oko still runs the review and says so.
- **`stado product cargo` builds a dependency checkout older than the manifest's pin from the
  pin:** a Git dependency is built from its canonical checkout, so a checkout whose `HEAD`
  predated the pinned `rev` compiled the dependency older than the manifest expects and failed
  with unresolved imports in the depending crate (`wisent_errors::Operation` in Stado). A build of
  a committed revision (`--revision`) now builds that dependency from its pin, fetched into its
  checkout when absent and exported beside the revision; a build of the working tree is refused
  first, naming the checkout, its `HEAD`, the pin, `git -C <checkout> pull --ff-only` and
  `--revision`.
- **An install on a machine with no registry document recycles no unit instead of failing:**
  `release local converge`, the Stado CLI recipe's first `after_install` step, read the machine's
  registry record to find its units and refused when there was none, so every install of Stado
  into a home outside any fleet failed after its build passed with `cannot read registry unit
  ownership: no registry document at …`. Such a machine declares no managed unit; the recycle now
  says so and the install goes on. A record that cannot be reached or read still fails it.
- **A committed build compiles every Git dependency at its pin:** `stado product cargo
  --revision` still compiled a dependency from its canonical checkout when that checkout's `HEAD`
  carried the pin, so another session's uncommitted edit there (`autoversion`'s working tree)
  reached the build and failed it with errors in Stado's own code. A committed build now compiles
  each pinned dependency from its pin, exported once beside the revision, whatever its checkout
  holds; a build of the working tree still reads the checkout.
- **A builder that does not answer no longer fails a whole build submission:** declaring a
  product's `secret_env` on every builder of its platforms ran `config show` on each one, so one
  builder asleep (`ssh: connect to host … Operation timed out`) refused every submission of the
  product. Each builder's host channel is now probed first; one that answers on no path is left
  out, named on stderr and under the step's `skipped`, and the submission goes on. A platform none
  of whose builders answers is refused, naming each.
- **A source install of Stado names its own revision to the build:** a release recipe's build got
  `WISENT_SOURCE_COMMIT` from the installation but inherited `STADO_SOURCE_REVISION` from whatever
  launched it, and Stado's `build.rs` refuses the two disagreeing (`STADO_SOURCE_REVISION must
  match authoritative WISENT_SOURCE_COMMIT`), so an install launched from a build job or another
  Stado's finish failed. The installation now sets both to the revision it builds.
- **Two waiting installations of one surface both finish:** an installation lets the surface's
  lock go while its after-install steps run, so another installation with `--wait` took it at once
  and recorded itself, and the first one's outcome was then refused (`another installation of
  stado/cli recorded itself while this one's after-install steps ran`). With `--wait` an
  installation that finds another still running its after-install steps now lets the lock go and
  waits for that process to end (a `czekam:` process wait ended by the kernel's exit report), then
  takes the lock again.
- **A release quarantined for a taken port is rolled out again once its ports are free:** a digest
  quarantined for `stable_bind_occupied` stayed refused until a person cleared it, so a vault's
  release stayed quarantined with its stable bind and both candidate ports free and every vault
  read refused. The release agent now asks the kernel who listens on those ports on each pass:
  with none but the release's own active process, it retires the record itself, writes its line in
  the audit trail and rolls the digest out again; while one is taken, `detail` names the pid and
  port.
- **Every journey selects a Stado config that exists:** 22 Rust journeys named a `STADO_CONFIG`
  under their own run root that they never write, which Stado refuses (`STADO_CONFIG names <path>,
  which does not exist`) and the configuration readers turned into a panic before the journey
  tested anything (`alerts-skrzynka` among them). Each now selects `tests/config/empty.json`, the
  empty config `tests/config/README.md` names for a journey that must read no operator
  configuration.
- **A configuration Stado cannot use is refused, never a panic:** a `STADO_CONFIG` naming a file
  that does not exist, or a malformed config file, reached the first configuration accessor a
  command called and ended the process with `thread 'stado-cli' panicked … invalid stado config
  file: Selected { … }` (`stado alerts preferences show` among them). Every command but `stado
  config` and `stado credentials migrate`, which write the selected file themselves, now refuses
  first as a configuration failure: `Error: STADO_CONFIG names <path>, which does not exist`.
- **Every spilled changelog part is linked, and an unlinked one is refused at push:** twelve parts
  under `changelog/unreleased/` were linked from no `CHANGELOG.md` line, and the version bump
  gathers only the parts the Unreleased section links, so their entries would never have been
  released. They are linked, and `stado quality check` now refuses a revision whose
  `changelog/unreleased/` holds a part the section does not link, naming each and the link to add.
- **A workload never reads the janitor's turn request or a hold record half written:** both were
  written by truncating the file and then filling it, so a build that read the turn request in
  between was refused with `cannot parse the janitor turn request …: EOF while parsing a value`.
  Both are now written whole (a staging file renamed into place), and the holds listing reads only
  `.json` records.
