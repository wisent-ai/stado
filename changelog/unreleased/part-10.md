# Changelog Unreleased (part 10)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **The beacon says which declared role the host process does not run, and the resident unit is
  rendered again for it:** the `roles` block carries `declared_not_running`, the `stado serve`
  roles the registry declares for the publishing process's own serve unit (the declared serve
  lines taking its role lock) that the process was not started with, read from both command lines
  by serve's own definition, with the units compared and any line the parser refused;
  `{"no_declared_unit": <lock>}` when no declared line takes the lock. When roles are missing and
  the process is the host's resident unit, the pass starts the same detached `service ensure` the
  executable starts when its unit's argument vector is refused, records the line it was started
  for, and starts none again for that line; `converge` says what happened. A unit rendered without
  `--resolver` keeps serving its API while every reader of the registry it would refresh goes
  stale; `stado host health` now prints `roles: declared but not running: --resolver (<unit>);
  rendered again: …`. A beacon collected by hand carries `null`. Journey:
  `tests/host_health/beacon/declared_roles.rs`.

- **`stado cloud host create|list|delete|reap --provider azure` rents fleet hosts from the cloud
  under the operator's credit floor:** `create NAME --location L --size S --disk-gb N
  --idle-minutes M` prints the projected hourly burn (the new machine's on-demand hour and that
  of every cloud host already running) and holds the create to the Azure credit gate with all of
  them counted, measures the public address of each host `cloud_hosts.admit_from` names (each asks
  the product site's `/api/egress` over its host channel) and admits only those to SSH, mints the
  host's fleet key as `fleet key generate` does, creates the public address, security group,
  interface and VM (cloud-init installs the build toolchain and keeps sshd key-only), enrolls it
  through the verified `fleet enroll` path, records `targets[].cloud`, waits for the first boot
  to end and reports the toolchain it answers. A host that cannot be enrolled is deleted again.
  `reap [--apply]` readmits the fleet's current addresses and deletes each host whose run area has
  stood empty longer than its idle minutes; the coordinator runs the applied pass on every tick
  when Azure is enabled. `delete NAME` deletes the VM, disk, interface, security group and address
  and only then the registry entry and the fleet key.

### Changed

- The web edge builds its address, security group and interface from the same Azure request
  bodies as cloud hosts (`providers::azure::public`), and its delete now follows each Azure
  operation to its end before confirming the resource is gone.

### Fixed

- **A throttled Cost Management query no longer leaves the Azure balance unread:** on HTTP 429
  the reader holds the longest wait Cost Management names (`x-ms-ratelimit-…-retry-after` or
  `Retry-After`), announced as a wait, and asks again; a 429 naming no wait is refused with its
  body. Before, one throttled read made every Azure create refuse for an unknown balance.
- **An Azure machine size is priced as a VM, not as its Cloud Services (classic) role:** the
  Retail Prices API lists both under the same `armSkuName`, and the credit gate took the higher
  one (5.696 instead of 2.752 USD an hour for `Standard_D64as_v5`). Only products named
  `Virtual Machines …` are quoted now.
- **`stado repair skarbiec --step crypto` runs Skarbiec's `keyring recover`:** Skarbiec 0.4.16
  renamed `recover-daemons` and made it end every gpg child stranded on the stopped agent; the
  payload still called the old name, so on a 0.4.16 vault owner the repair could not run, and on
  0.4.15 it replaced the daemons while the stuck gpg children kept every item read waiting.

- **`stado host upgrade TARGET` brings a host's Stado to a newer committed revision:** a host
  whose own Stado cannot read the registry authority, or holds no checkout to build from, had no
  product path to a newer release, so each one was upgraded by a hand-written sequence. The verb
  fetches origin/main without asking for a credential (or takes `--source-commit`, which
  origin/main must carry), refuses a revision older than or beside the one the host runs, builds
  it on a host of the target's release platform (`--builder`, else this machine when it shares
  that platform, else the target), signs it here with the fleet's signing role or
  `--signing-keychain`/`--signing-identity`, delivers it into a run of its own, lets the
  incoming program judge the host's configuration (`config validate`) and install it
  (`release local install`), and reads the registry, the host's own registry read, its vault
  (`skarbiec doctor`) and every declared service at its port before and after. A check that
  answered before and no longer does puts the kept previous program back and says so. When the
  signature changes the program's code identity, the verb lists each macOS dialog the host and
  this Mac will show with its answer, runs only from a terminal, and starts when the host's
  name is typed back. `--join-network` joins the host to Stado's network afterwards; `--plan`
  reads and reports every step and changes nothing. Stado Desktop has both the plan and the
  upgrade.
- **`release local install --announced-identity REQUIREMENT`:** an install over the host
  channel that changes the installed program's code identity was refused for want of a terminal
  even when the caller had told the person at the Mac what macOS would ask; it now installs when
  the candidate's designated requirement is exactly the one the caller announced, and refuses any
  other change as before.
- **`release local restore --kept-previous SHA256`** puts `~/.stado/bin/stado.previous`, the
  program the last install replaced, back byte for byte when it has that digest, and recycles
  the units running the image it replaces: the rollback for a host whose previous Stado no
  delivery retained as an archive.
