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
