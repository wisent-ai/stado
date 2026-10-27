# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

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

- [Unreleased (part 1)](changelog/unreleased-part-1.md)
- [Unreleased (part 2)](changelog/unreleased/part-2.md)
### Fixed
- **`stado release newest --plan` ignores non-directory entries:** regular files and symlinks are not checkout directories; an entry whose type cannot be read still refuses with its path and filesystem cause. Journey: `tests/release/newest/push-order.mjs`.
### Added
- **A host lends its idle capacity to another Stado fleet:** `stado market`
  takes `--provider stado --peer NAME` beside `--provider vast`. `stado market
  peer add|edit|remove|list|show` declares the other fleet (its object API
  origin, queue namespace and the credential role whose `token` is the bearer
  it issued). `auto-list --provider stado` runs a standalone Stado worker
  (`--kind stado-peer`, a home of its own, only the peer's store in its
  configuration) against the peer's queue once ours has been idle for the
  window, and drains it when our work appears, the operator unlists, an
  inference reservation holds the cards or `--max-duration-s` passes.
  `list`/`unlist` flip the host's offer for the loop that holds the peer,
  `status --peer` reads what each host lent from the peer's own job records at
  the offered price, `status` alone reads what peer workers ran here, and
  `readiness` asks the channel, the vault and the peer's store
  (`stado.peer-readiness.v1`). A worker now drains on request
  (`~/.stado/drain` in its home): it claims nothing more, publishes
  `drain_requested` as its admission reason, and exits once its jobs finish.
  `--price-disk` is now required only with `--provider vast`. Stado Desktop's
  Earning screen carries the same operations under **Other Stado fleets**.
- **`stado host activity TARGET [--since TIME] [--json]` says what a host
  did:** every job of the queue whose worker allocation names one of the
  target's registry hostnames or its name, oldest first, from the records
  under `running/` and the terminal prefixes, with its times, lifecycle,
  state, command, worker kind, measured seconds and error; a time that is not
  RFC 3339 is named, never replaced, and records naming no worker are counted
  per prefix. Stado Desktop's Hosts inspector shows it under **Activity**.

- **Most is a catalog service:** `stado service ensure most --host <host>`
  runs `most-server` on the loopback port the service directory assigns,
  checks `GET /healthz`, and files it, so `stado service directory connect
  most --consumer <consumer>` resolves where Most runs.
- **`stado resolver remembered show|forget [--json]`:** `show` prints what
  this host remembers about reading its remote registry authority (a read
  that got no snapshot, which turns readers to their copy until the refresh
  interval passes, and the kept snapshot); `forget` drops both so the next
  command asks the authority at once. Stado Desktop runs both, and its
  "Give this host's resolution API a port" operation now runs `resolver api
  reassign` (it ran a verb the CLI does not have).
- **A placement move checks the destination has room before fencing:** the
  preflight measures each state on the source (`du -sk`) and the free space
  of the destination volume under each state root (`df -Pk`), and refuses the
  move when the states written there need more, naming both figures, instead
  of failing at the first write after the source is already down.
- **`stado registry pull --last-known-good [--path P]`:** prints this host's
  last-known-good copy without asking the authority or the store, so the
  authority and generation this host holds can be read while both are down;
  Stado Desktop reads it too.
- **`stado placement profile draft`:** prints the placement profile that would
  move services declared as fixed routes, each unit read from the host that
  runs it, to pipe to `profile declare --file -`. Stado Desktop runs it.
- **`stado placement move … --check`:** every check a move makes before it
  fences the source, against the real hosts, claiming nothing; `credentials
  vault move` runs it first (`placement_check`) and, with `--apply`, stops
  before rotating the owner when it would be refused. Stado Desktop runs it.
- **`stado placement evacuate --from-host A --to-host B --reason WHY
  [--vault-owner-uid UID] [--authority] [--standby] [--profile-fixed-routes]
  [--check]`:** empties a host in one command: the vault (through
  `credentials vault move`), every profile, every role of the host Stado
  process, and the authority last (on B), after every step's check passed,
  refusing when the registry still places anything on A after. `--standby`
  stands B by where it does not fully stand by; `--profile-fixed-routes`
  gives each fixed route the profile `draft` prints first. Desktop runs both.
- **`stado placement standby … --check`:** gives every refusal the pass gives
  before it delivers anything (an unknown or held profile, the placed host,
  a service the catalog does not know, a program no delivery path reaches)
  and lists what it would deliver, delivering, staging and writing nothing.
  Stado Desktop runs it.
- **`placement move --authority` hands the role over through an authority
  that answers:** it commits the registry naming the new authority at the
  old one, then writes it into the new host's copy and store; a rerun after
  the commit finishes the rest. It used to refuse, pointing at a `placement
  move` on the old authority that has no way to move the role. The receipt
  says `how`: `handed-over` or `taken-over`; `--check` says which it would be
  and writes nothing.
- **`placement standby` finds the catalog product a directory service runs**
  through the unit the placed host declares for it (`weles-admission` runs
  `com.wisent.weles`); such a profile was refused as unknown to the catalog.
- **`stado storage fence arm|disarm|status`:** arms the write fence of this
  host's local object store for a named transaction, after every write in
  flight has finished, so a copy taken then holds every accepted write;
  lifts only its own transaction's fence; a refused write names the fence
  and how to lift it. The first step a move of the object API between hosts
  needs. Stado Desktop runs all three, and `stado resolver status` reports a
  fenced store as a blocker.
- **`stado placement move-role SERVICE --to-host HOST --reason WHY
  [--check]`:** moves a role of the host Stado process — the fleet object
  API — which no placement profile can carry: checks room for its store on
  the new host and this machine, fences the store on the active host, copies
  `~/.stado/local-storage` to the new host, and switches the route in one
  registry commit, the new host's serving address becoming its endpoint; a
  failed step puts both hosts back, and the old copy stays fenced after the
  move. `--check` stops before the fence. `profile draft` points at it. Stado
  Desktop runs it and its check.
- **`stado placement history [--json]`:** every profile move, role move,
  authority move and evacuation run this machine ran, newest first, with its
  receipt or the refusal that stopped it, kept under
  `~/.stado/placement-history/`; after an evacuation it says what moved where
  and which step stopped. Stado Desktop runs it.
- **Stado Desktop lists the CLI paths it runs:** `Stado --operation-paths`
  prints every capability operation's group, id, path and fixed arguments
  as JSON, and a desktop API test runs each path's `--help` through the built
  `stado`, naming any operation whose verb the CLI does not have.
- **A chunked object upload prints each part's time:** an upload through the
  object API that is sent in parts prints `stored part <n> of <total> of
  <uri>: <bytes> bytes in <time>` on stderr as each part is stored, so an
  upload that crawls names where it waits.
- **A catalog service declares the stores that move with it:**
  `service.state` in `catalog/products.yml` lists paths under the account's
  home, each a file or a `tree`, `required` or not; `placement profile
  draft` adds them to the profile it prints, `placement profile sync-state
  NAME` adds the ones an existing profile lacks, and a placement move (and
  its `--check`) refuses a profile that lacks any, naming `sync-state`, so a
  service moved to another host finds what it kept. Skarbiec declares its
  vault, acquisitions and audit files; Brama its donated subscriptions and
  `.brama`; Weles its keeper store and detached run records. Stado Desktop
  runs `sync-state`.
- **`stado placement holds [HOST] [--json]`:** what the registry places on a
  host — its placement profiles (the vault's marked), host Stado roles,
  whether it is the authority, its fixed routes — and whether an evacuation
  emptied it; without HOST one line per host. Nothing changes. Stado Desktop
  runs it for the selected host.
- **`placement standby` finds the release control that rolls a tree out:**
  the product of the tree's name, or the one whose `service` names it (Weles'
  admission tree is `weles-worker`'s), its root moved under `{home}`, and
  installs the verified desired release where the catalog program runs when
  that release control installs elsewhere, so the staged unit can start.

### Changed

- **Changelog moves share a writer and validate part identities:** empty outlines do not become releases or block submission, and empty linked parts are collected.
  Desktop describes the same operation without copying backend-owned directories and retains the CLI's refusals.
- **A placement move stops an empty copy on its destination:** a destination
  unit already running is refused as a second copy only when the destination
  holds some of the profile's required state; one holding none (a loaded
  standby, like the laptop's Skarbiec with no vault file) is stopped first.
- **`registry doctor` names a `foreign-install-root`:** a release-controlled
  product rooted under one target's home while another target's home differs,
  with the `{home}` form to write through `stado release policy apply`.
- **`placement standby` probes a host where its unit listens:** the probes it
  writes for the new host take the port the directory names for that host
  (else the placed host's port, which the unit then takes), keeping scheme,
  host and path; they were copied verbatim from the placed host, so a host
  the directory already served on another port was probed on a port nothing
  listened on there, and the move's health check judged the wrong service.
- **`placement profile draft` probes from the catalog:** a service drafted
  without `--probe` is probed with the catalog's `answering` request for the
  product its unit runs, when that is a GET with no body, at the address the
  directory names for it on its active host; `--probe` is needed only where
  the catalog declares none. The catalog now declares Weles' `GET /healthz`,
  which the host Stado process also asks on each beacon pass.
- **The product catalog states no number of its own:** the host Stado
  process's beacon period is the value the host's registry entry declares as
  `health_interval_seconds` (`${declared:<field>}` reads any field of the
  target's entry, and a host that declares none is refused naming the field
  and `stado registry set --path targets.<host>.<field> --value <value>`);
  bind addresses are written as `$STADO_LOOPBACK` and `$STADO_ALL_INTERFACES`;
  sources cite files, not line ranges; and the catalog carries no
  `schema_version`, which it only ever compared with itself.
- **A remote registry authority is read once per refresh interval, not once
  per command:** the snapshot a command read is also kept in
  `~/.stado/authority-snapshot.json` (`STADO_AUTHORITY_SNAPSHOT_FILE` moves
  it) until the interval ends, and the next command answers from it, saying
  so on stderr; a registry write forgets it. A registry write to an
  authority whose read was given up less than an interval ago is refused at
  once with that read's sentence instead of standing on the silent host.
  `stado resolver resolve`, the relays and the database plane no longer read
  the authority a second time after the store read: they take this host's
  resolver's held generation or the kept snapshot first. An authority no SSH
  path can reach at all is remembered like a read given up, so the next
  commands in the interval go to their copy at once instead of each standing
  the same connection timeout; the refusal now reads `… a read of it at <time>
  got no snapshot (<cause>), and it is asked again once the <N>s refresh
  interval has passed`. A registry store that is an object API placed on that
  same silent host is not asked either: the read goes to the last-known-good
  copy at once instead of standing the relay's own wait.
- **`stado placement transactions` removes spent move records:** the
  progress records this host keeps of moves the registry no longer holds open
  are removed and named on stderr; a record whose move process still runs is
  kept.
- **Files Stado keeps are found under the same home and written durably:**
  agent grant renewal, release-agent retirement, the owner host password
  read, token custody, onboarding progress, observations, the self-update
  attestation copy, the storage-transaction directory, fleet database REST
  placement, file retirement, image scans and Jeden attachment read the home
  as every other Stado file does (HOME, else the user database), so a unit
  started without HOME no longer refuses; token custody, onboarding progress
  and observations are written whole and flushed with their directory.
- **Token custody no longer bounds a bearer at 4096 bytes:** the bound was
  not Skarbiec's, which reads a fixed token of any length; the bearer is
  verified against its grant's hash as before.
- **Whether a pid is a process is one answer:** local slot admission and
  queue lease owners ask the shared check, so a slot whose process belongs to
  another account is no longer read as gone.
- **`stado resolver status` says what this machine remembers of the
  authority:** `authority not asked: …` while a given-up read stands and
  `authority snapshot kept version=<v> until=<t>` while a kept snapshot
  answers registry reads (`--json`: `authority.remembered`).
- **The object API recovery's readers refuse instead of guessing:** a host
  config that cannot be read, a store prefix that cannot be resolved, a
  non-UTF-8 variable or an unreadable server state is refused naming it;
  relative store, token and state paths are resolved under the home the
  recovery runs as, not launchd's working directory.
- **`stado service env write` and storage-root transactions resolve `$HOME`
  as every Stado file does**, and the storage-root transaction, its rollback
  unit and the release agent refuse a home that is not UTF-8 instead of
  writing it mangled.
- **Report paths are shortened to `~/…` only under the home itself:** a
  sibling directory whose name starts with the home's is printed whole.
- **`stado cost report` reads a job's model the way admission and sizing
  do:** `--model=<value>` is read (it was reported as `=<value>`) and
  `--model-path` no longer counts as a model; a row whose command names none
  carries `model: null` in `--json`, and the run-time medians the local pack
  and the autonomy optimizer use are keyed by the same reading.
- **A placement move transfers and probes at once:** every state of the
  profile is read at once on the fenced source and written at once on the
  destination, and every health probe is read at once after the units start,
  so a profile with several stores or services stands its longest step, not
  their sum; the preflight asks the source's probes, state files and routing
  units each group at once too, `placement standby` delivers every managed
  program at once, and `placement move --authority` hands the promoted
  registry to every other host at once. Each step is still its own timed wait in the
  receipt, and the first failure in the profile's order is the one reported.
  A rollback puts every destination state back at once, and a committed move
  settles every state backup at once. A single-file state
  is streamed through its encoder and decoder instead of being held whole in
  a shell variable or quoted word, so a store of hundreds of megabytes moves,
  and an encoder that fails on the source refuses the read.
- **The config file and Stado's records survive a power loss:** `config
  set`/`unset`, the database and web planes and the identity migration
  replace the config through one writer that keeps its permission bits and
  flushes the file and its directory (`config set` prints `(absent)` for a key
  that was not set); the resolver's published state, its vault consultations,
  held schedules, inference plans, the release agent's rollout state, the
  agent's placement policy, the local credential store, the credential-store
  selector, peer bearers, held repair keys and forward markers are written the
  same way, and no two writers share one fixed temporary name any more. A host
  with no home no longer stops when it would keep a repair key. `stado
  self-update` puts the new binary and its attestation copy in place the same
  way, streamed from the verified file rather than held in memory, so a host
  that loses power after the swap starts the new binary. `stado service file
  fetch` writes the fetched file owner-only from its first byte (its staging
  file was readable by others until a chmod), and the web edge's kept
  Caddyfile and the recovery config install go through the same writer.
- **The Desktop API names the command it does not have:** `POST
  /api/operator/run` reads the command words below the family against the
  CLI's own tree and refuses one that names no command with HTTP 400, `stado
  <words> has no command "<word>"; it declares: <commands>`, before any model
  judgement or run.
- **`stado service list --unowned` and `--undeclared` ask every local host
  at once:** a host that does not answer no longer holds the scan of the
  others; rows keep their order.
- **`stado release status` reads every target's release state at once:**
  each is its own object read, and against a slow store they stood the
  store's wait once per target; the report keeps its order.
- **`stado resolver status` names an authority that reads another host's
  store:** a host that is the registry authority by its own copy while its
  registry store is an object API placed on another host gets a blocker
  naming both, the state a takeover leaves until the object API moves too.
- **Units and paths on a host use the home that host states:** `placement
  standby`'s release-control declaration, `service ensure`'s rendered unit
  and acquisition scopes, the reconciler's repaired unit, and the unit file
  of a catalog product just ensured take `$HOME` from the host's own shell,
  read over its channel, instead of `/Users/<ssh user>` or `/home/<ssh
  user>`; a host that states none is refused as unreachable.
- **A move to a host the profile does not name says how to add it:** the
  refusal now ends `prepare it with stado placement standby <profile> --host
  <host> --reason <why>`, for `placement move`, its `--check` and `evacuate`.
- **A placement move no longer carries the fleet vault past its owner key:**
  `placement move` and `placement evacuate` refuse the profile carrying
  Skarbiec, naming `credentials vault move`, which rotates the owner first
  and passes `--vault-owner-rotated` to the move it runs.
