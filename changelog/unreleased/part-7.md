# Changelog Unreleased (part 7)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **`stado fleet attach NAME`, run by `stado bootstrap --target NAME` (and `fleet enroll
  --bootstrap`), attaches a machine that runs its own standalone Stado to the fleet store:** it
  authorizes the machine's resolver key on the directory authority, declares the machine's resolver
  adapter for the object API exactly as this host declares its own (ports the machine hands out),
  writes the bearer of the fleet namespace (the item `object_api.namespaces` names, read as named)
  to `~/.stado/queue-object-api-token`, gives it this host's `ssh.keepalive_*`, and sets its
  `storage` to the store this host reads. Its device-local store stays at the path its
  configuration names and is reported. Before, such a machine's `serve --auto` resolved the name
  its own store gave it, refused the registered one, and published no beacon.

- **`stado host processes TARGET [--json]` names what each process on a host spent:** a saturated
  host could only be read with `top` or `ps`, which an agent session, a sandbox or the host-exec
  allowlist can be refused, and `ps` counts CPU in whole seconds. The host's own Stado now reads
  every process's CPU seconds, the CPU of children it reaped, resident memory and bytes read from
  and written to storage from the kernel, and reports each against the reading it kept before,
  with the interval measured; the first run says it has nothing to compare with yet. Every beacon
  carries the same report as `processes` (`{"unread": <cause>}` when the kernel refused the
  listing). Real test: `cargo test --manifest-path stado-rs/Cargo.toml --test scheduler --
  host_processes::`.

- **A host routine runs under its job identity and keeps its output:** the host store's runner
  started a routine with no directory or job identity and kept nothing it produced, so a routine
  that needs `WC_JOB_ID` or hands back its receipt as an artifact failed or left no receipt. Each
  host job now runs in `~/.stado/host-store/work/<job-id>` with the fleet worker's job environment
  and its `output/` is kept under `status/<job-id>/output/` in the host store when it ends.

### Fixed

- **`stado registry push - --last-known-good` hands the fleet registry to a standalone Stado being
  attached:** it refused every host whose copy named no authority, which is every standalone Stado,
  so an attached machine kept its own registry, resolved the name that registry gave it and never
  started the resolver that reaches the fleet. It now takes a document that names another host as
  authority and lists this host, on a copy without a service directory, and `stado fleet attach`
  ends by handing the machine the fleet registry this way.
- **`stado fleet enroll NAME --ssh DEST --install-key` enrolls a machine that has no fleet key yet:**
  Skarbiec answers the fleet's reader the same 403 for an item it may not read and for an item that
  does not exist, and enroll took that 403 as fatal. It now asks the vault owner's item list: an
  absent `stado-ssh-NAME` is minted; a present one is a mint cut off before its grant, which is
  completed and read again.
- **The `--install-key` password prompt is shown and answered:** the first-contact `ssh` ran as the
  leader of its own process group, so opening the terminal for OpenSSH's password prompt stopped it
  (SIGTTOU) and enroll waited without end. It now runs in Stado's own process group, through the
  announced wait.
- **Vault writes on another host use `skarbiec set-json`:** `stado credentials item put`, every key
  mint stored on the vault owner and the owner-vault writes called `skarbiec set`, which current
  Skarbiec no longer has, and were refused with `unknown command: set`.
- **`stado bootstrap` and `fleet enroll --bootstrap` fail when a target was not provisioned:** a
  failed install was printed as `[err]` and the command still exited 0, so enroll reported the
  machine enrolled. The run now refuses, naming every target that failed and why.
- **Bootstrap installs the Stado version the registry declares for the host:** it downloaded the
  configured `release.version` even when the host declares `managed_versions.stado`, and says which
  of the two it used.
- **A host Stado unit replaces its renamed API predecessor:** the predecessor runs `stado serve` and
  held the role lock, which refused the new unit before the API takeover that retires the
  predecessor could run, so the unit restarted beside it without end. When the role is held, the
  takeover now runs before the lock.
- **Bootstrap downloads Stado from the fleet's declared release origin:** the installer fetched from
  `api.url`, which can name a site that serves no release object at all (every download answered
  404). It now uses the `public_origins` declaration that publishes `/api/release/object`, and says
  which origin it used.
- **Bootstrap reads the workload-agent grant this host declares:** it refused unless
  `agent.skarbiec.consumer` was one fixed name. It now hands over the declared consumer and bearer
  file when they are distinct from the control plane's own, and otherwise provisions the machine
  without one, saying so: the machine runs jobs that declare no secret and leaves the others queued.
