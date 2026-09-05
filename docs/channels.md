# Channels

A Stado channel is a declared connection between two managed parts of the fleet. It is not an SSH session, a copied token, or an operator-maintained tunnel. The registry names both endpoints, Stado checks the relationship, and the host channel carries any repair without returning secret values to the caller.

## The four boundaries

| Boundary | What crosses it | Source of truth | Commands |
|---|---|---|---|
| host channel | fixed programs and scripts over the target's declared transport | registry target plus host key | `stado host exec`, product-specific `stado host` commands |
| service directory | service name to endpoint and consumer relationship | registry service declarations | `stado service directory show`, `stado service directory connect`, `stado service verify` |
| object authorization | object URI plus action; bearer remains local to the caller | `object_api.namespaces` and their Skarbiec items | `stado storage stat|get|put|ls|rm`, `stado host reconcile-object-verifier` |
| workload secrets | one exact item field delivered to one service environment | Skarbiec grant plus managed-service declaration | `stado service grant-sync`, `stado service auth-check`, `stado service secret-sync` |

These boundaries are separate. A host may answer while a service relationship is wrong. A service may listen while its bearer is stale. The object API may be live while its verifier cannot read namespace credentials. Health therefore means the final operation succeeds, not merely that a process or TCP port exists.

## Host channel

The host channel resolves the target from the canonical registry, checks its pinned host identity, and runs a fixed operation through Stado. Anything absent from the command allowlist belongs in a checked-in Stado operation; it does not justify raw SSH.

Use `stado host exec <target> <allowlisted-command>` for read-only diagnostics. Mutating workflows use their owning commands, such as `stado service file-sync`, `stado service secret-sync`, `stado release apply`, or `stado host reconcile-object-verifier`. Secret values are read and written on the target; they never appear in the remote argument list or command result.

Set `RUST_LOG=stado::deploy::host_channel=trace` when the same operation needs OpenSSH's own transport diagnosis. Stado adds `-vvv` to the shared SSH invocation only while that existing tracing target is enabled; normal argv is unchanged, and the debug stream remains verbatim in the operation's existing stderr and status receipt.

An unmanaged executable is retired with `stado host retire-file <target> <absolute-path> --product <product>`. Add `--dry-run` for an exact-path preflight: it reports a transaction token, exact planned destination, byte count, mode, and SHA-256 without creating a directory or moving the source. A reviewed apply carries that receipt back with `--transaction`, `--expected-sha256`, `--expected-size`, and `--expected-mode`; all four must be supplied together, and Stado refuses before destination creation when the current source differs or the transaction token is invalid. Stado Desktop always uses this receipt-bound form. User executables must be owner-owned regular non-symlink direct children of `$HOME/.stado/bin`, `$HOME/.local/bin`, or `$HOME/.cargo/bin`; they move atomically into the product backup tree. One exact root-owned `/Library/LaunchDaemons/*.plist` is also accepted under the target's approved sudo grant and moves atomically to a non-loadable sibling. Retire a legacy plist and its convenience binary as two separate reviewed operations.

### Retiring an undeclared init-system unit

`stado service bootout <exact-unit> --host <target> [--domain system|user]` is the declaration-independent stop path for a unit found by `service list --undeclared` or `service label-print`. It addresses the exact launchd label or systemd unit name supplied; it does not derive a prefix or add or remove a `.service` suffix. With no domain, Stado preserves system-first precedence and checks the calling account only when the system manager holds no exact unit by that name. Pass `--domain user` when the same or a related canonical name must remain running in system scope.

On Linux, bootout uses the target account's explicit systemd user bus for `user` scope and the existing non-interactive privilege path for `system`. It runs `systemctl disable --now` for only the exact requested identity, then refuses unless that identity reads back inactive and not enabled. An absent identity is a retry-safe `absent`; manager, privilege, disable, identity, and postcondition failures are `refused`. On Darwin, the existing system or per-login launchd bootout and absence checks are unchanged. Neither platform branch deletes the unit definition; file removal remains a separate `stado host remove-file` operation.

For example, the obsolete Ubuntu user unit `com.wisent.compute.service.stado-resolver.service.service.service` can be retired with an explicit user-domain bootout. The canonical `com.wisent.stado-resolver.service` is a different exact identity and is not selected or changed.

### Keeping a service's non-secret environment

`stado service ensure <name> --host <target> --env NAME=VALUE --reason <reason>`
records each non-secret assignment in the managed service's `env` map and renders
it into the host unit. Repeat `--env` for multiple keys; the last assignment wins.
Recorded values override catalog defaults and survive subsequent `ensure` calls
and automatic repairs. `$HOME`, `$STADO_HOST`, and `$STADO_PLATFORM` expand
against the target, not the caller. Credentials still use `secret-sync`, never
`--env`.

If a loaded unit's arguments match but its rendered environment differs,
`ensure` saves the prior definition and loads the new one. On macOS this requires
bootout and bootstrap: kickstart alone would reuse launchd's old environment.
If the new definition fails to start, `ensure` restores and starts the previous
definition and reports failure; it never reports the requested environment as
applied after rollback. A different executable or argument vector remains an
explicit conflict on a loaded macOS unit.

For an independently managed instance outside a release policy's target map,
`registry doctor` accepts pinned environment only when both the registry record
and the local unit file contain the product's exact required values. A remote
unit that was not read is not treated as agreeing.

Run `stado host exec <target> -- stado registry doctor` to measure those
host-local facts through Stado. The fixed read-only command uses the target's
installed binary and returns its own registry and executing-image findings;
running the doctor on a workstation cannot establish them for another host.

### Release-controlled placement handoff

A service placement unit has one exact lifecycle. Stado-managed units retain the existing shape:

```json
{"name":"skarbiec","unit":"com.wisent.always-on.skarbiec","path":"/Library/LaunchDaemons/com.wisent.always-on.skarbiec.plist","kind":"launchd"}
```

A logical service whose process lifecycle belongs to the release controller instead carries only:

```json
{"name":"skarbiec","controller":"release-control","product":"skarbiec"}
```

The external shape must appear in every host template for that service and name one release product. The service directory still owns its route, active host, endpoint, consumers, and generation; the placement profile still owns state, routing units, and probes. Routing units remain ordinary Stado-managed units. Static validation requires the route's active host, endpoint, active-host probe, product, release target, and stable bind to agree; it rejects a target service row or any legacy launchd restore field on every target of the owning product. Resolution and verification therefore keep using the route without pretending the external process is a target-managed unit.

Install 0.15.24 or newer on every registry reader and agent before publishing this shape. Then run `stado service handoff-release-control <service> --host <active-host> --product <product> --json` once. The command proves the committed desired release, active signed executable, stable proxy, readiness, inactive legacy unit, legacy file digests, and absence of an executable caller before one generation-bound compare-and-swap externalizes all templates, removes the active target row, removes the legacy restore identity, and advances the release and directory generations. It preserves the route, endpoints, consumers, profile state, routing units, and probes. Every placement mutation refuses a release-controlled member before a transaction or host action.
A handoff invocation first fsyncs a version-scoped `prepared` receipt under `~/.stado/work/service-release/<product>/<version>/`, then performs the sole registry compare-and-swap and immediately advances that receipt to `registry_committed`. If the caller is interrupted, rerun the identical command: a matching prepared receipt is reused only after the same intent, exact legacy files, active release, and still-managed lifecycle are re-proved; a registry that already carries the intended handoff causes the command to skip the successful CAS, reacquire the same per-service lease, and finish the reconciler-report fence. Do not delete or replace the receipt, issue a different handoff, or infer completion from the registry shape alone; `handed_off` with a satisfied fence is the retirement boundary.


After the successful handoff, retire the reported plist and convenience binary separately with `host retire-file`, plist first. Each handoff receipt supplies the unique transaction, SHA-256, byte count, and four-digit mode passed to the mutating command's four explicit binding flags; do not perform another dry-run or an unbound mutation. Each retirement reports `retired` or retry-safe `absent`, so a failure after the plist move preserves an exact partial-cleanup record and the binary operation remains independently resumable. Rollback after handoff is not a binary downgrade: it requires a new generation-bound registry change, restoration of both archived files, and proof that release-control is no longer serving. Older readers cannot parse the external shape and must not be reintroduced while it is published.

### Ordered connection paths

`ssh` remains the preferred host-control destination. A target may also declare up to 16 ordered `ssh_fallbacks`; each fallback has a stable lowercase name and an SSH destination:

```json
{
  "name": "charless-mac-mini",
  "ssh": "charles@192.0.2.10",
  "ssh_fallbacks": [
    {"name": "nebula", "destination": "charles@192.168.100.10"},
    {"name": "tailscale", "destination": "charles@charless-mac-mini.tailnet.example"},
    {"name": "lan", "destination": "charles@charless-mac-mini.local"}
  ]
}
```

The names describe routes, not transport implementations. Nebula, Tailscale, WireGuard, ZeroTier, a private LAN, and a public address all provide an IP path; Stado still supplies the host identity, credential, fixed operation, and audit boundary above that path. Public Tailscale Funnel endpoints remain service publication and do not become host-control routes.

When a target has more than one path, Stado authenticates a side-effect-free `true` command in declaration order and sends the real operation exactly once through the first path that answers. A one-path target keeps the original single connection attempt. `stado host link <target>` probes every declared path and reports the selected path, so a working primary cannot hide a broken fallback.

Manage the declarations without editing the registry document by hand:

```console
stado registry host path list charless-mac-mini
stado registry host path set charless-mac-mini nebula --ssh charles@192.168.100.10 --priority 1
stado registry host path set charless-mac-mini primary --ssh charles@192.0.2.10
stado registry host path remove charless-mac-mini nebula
```

`set` and `remove` accept `--json`; Stado Desktop uses those typed receipts rather than parsing terminal sentences. In the Desktop Hosts inspector, **Host-control routes** shows every declared destination and probe answer, marks the route Stado selected, and opens the same set/remove commands behind a review. **Beacon network path** remains separate because it describes how the host published its beacon, not how Stado reaches the host.

When a host answers through one of those routes but its beacon is stale, `stado host link <target>` also reads `com.wisent.host-health-beacon`'s declared log and returns a `beacon_publisher` diagnosis. The exact `verifier_unavailable` diagnosis is repairable with `stado host repair-link <target>`: Stado resolves the `stado-object-api` authority from the service directory, copies the authoritative `stado-host-health-api/token` value into that authority's target-local verifier shadow, adds its read to the existing least-privilege verifier grant without rotating either bearer, waits for the host's normal publisher to write a newer beacon, and closes the open silence. It restarts no service and refuses every other publisher diagnosis rather than guessing.

Stado Desktop shows **Repair beacon publication** only for that repairable diagnosis. The action runs the same command and keeps its success or refusal on the Hosts screen; a stale host with a different publisher failure remains diagnostic-only until its own exact repair exists.

`host exec`'s allowlist takes no operator-supplied path and exposes no arbitrary shell. Entries that inspect a path fix that path and every flag in source. Fixed Cargo metadata is a typed `stado host inventory <target>` section instead: it uses lstat for the managed account's `$HOME/.cargo` and preserves that entry's link target, then follows only its fixed `bin` child and inventories every direct child including dotfiles with type, mode, numeric ownership, size, modification epoch, and symlink target. This supports the ordinary layout where Cargo home is a link to a mounted cache without accepting an operator-selected path or opening a file body. `entries_seen`, `entries_complete`, `entries_state`, and the enclosing `complete` field make truncation, a refused or partial traversal, malformed metadata, or a sanitized name explicit. The CLI tables the same fields, and Stado Desktop's Hosts inspector requests that typed report on demand for one selected canonical host through the authenticated `GET /api/host/inventory?target=…` route; it does not spawn the CLI. The content-reading exceptions remain exact: the fixed OpenSSH diagnosis reads only `/etc/ssh/sshd_config` and the last 200 records of `ssh.service`, with the file, unit, line bound and pager mode all fixed in source. A managed unit's owner-controlled env file — the one a launcher `.`-sources, not the one the unit file declares — is read with `stado service env-show <service> --host <target> --env-file <path>`, through the same channel and the same `$HOME` confinement `stado service env-set` writes through. Values whose key looks like a credential, and URLs carrying userinfo, are withheld on the target and never cross the channel; endpoints, ports and variable references are shown, because those are what an operator must verify. `stado service endpoint-check` reconciles the loopback endpoints that file declares against the target's own socket table and exits non-zero when a declared dependency is dead.

A configuration surface Stado can write and cannot read is not a boundary, it is a blind spot: on 2026-08-30 a managed unit named a Skarbiec endpoint nothing served, two writes of the correct endpoint were reverted, and no command could show either fact. `stado service env-set` therefore reads the key back through the same channel after writing it and exits non-zero unless the file's effective assignment holds what it wrote. The comparison happens on the target, so a secret is verified exactly without its value returning.

### An env key can have an owner other than the operator

A managed env file may be reconciled by something already running on the host, and a write to a key that something else owns does not survive. On charless-mac-mini `com.wisent.compute.service.weles-release-cutover` (`$HOME/.stado/bin/weles-release-cutover`) deletes `^WC_SKARBIEC_URL=` from `$HOME/.config/weles/worker.env` and appends `WC_SKARBIEC_URL='<contents of $HOME/.stado/forwards/skarbiec.url>'`, and it also deletes `STADO_RELEASE_API_URL` while writing `STADO_RELEASE_LOCAL_ROOT` in its place. Those keys are declared by the marker and by that script, not by whoever last ran `env-set`.

This is why the read-back names the forward marker whose contents match what replaced the write: the repair is to correct the declaration, not to write the file again. `stado service list --undeclared` enumerates every unit a host has loaded and what each one runs, which is how such a writer is found when no marker explains it.

That writer is a FINISHED one-shot script that launchd will not let finish. Its unit (`$HOME/Library/LaunchAgents/com.wisent.compute.service.weles-release-cutover.plist`) declares `RunAtLoad` and `KeepAlive` with no interval, and nothing in the registry declares the unit at all. Each run reads its own completion marker and says `reconciling completed release cutover`, re-imposes its configuration stage — the env-file rewrite above — then fails at a later stage (`verified worker archive is missing its exact Skarbiec acquisition scope catalog`), restores the legacy checkout, and exits non-zero. `KeepAlive` restarts it, so the configuration stage is re-applied indefinitely. `stado host unit-log <target> <label>` shows that cycle verbatim.

Two lessons, both cheap to check and expensive to miss. A migration script under `KeepAlive` is not a migration, it is a reconciler nobody declared: `KeepAlive` suits a service that is meant to keep running, and a script that exits when its work is done needs `StartInterval` or no keepalive at all. And a loop that repairs by restoring is a loop that pins the past in place — this one restores a legacy checkout every cycle, which is why `$HOME/weles/scripts/worker/deploy/launch-mac.sh` is still the program three units execute on that host even though the Weles repository deleted that file on 2026-08-24. At the time neither `weles-release-cutover` nor the launcher it restores was contained in any repository, so neither could be reviewed, diffed, or reproduced from source; the cutover script has since been recovered byte-exact into `deploy/weles-release-cutover` (below). The two need opposite repairs, and telling them apart matters: the cutover script is live operator tooling and belongs under version control, while the launcher was retired on purpose and must not be resurrected to be patched — the host has to converge onto the current release instead.

That last point has a sharp edge worth stating, because it is what makes a restoring loop worse than a stalled one. The configuration the host is failing on had ALREADY been corrected upstream: the launcher it restores gates unconditionally on `STADO_RELEASE_API_URL`, while the Weles repository's live source accepts either that or `STADO_RELEASE_LOCAL_ROOT` and no longer contains the launcher at all. The loop is the reason that correction never arrived. A host pinned to a deleted file does not merely stop improving; it keeps failing on a defect that no longer exists anywhere anyone would think to look.

### Retiring that reconciler, and the one command that puts it back

On 2026-08-30 the loop was retired. `stado service adopt com.wisent.compute.service.weles-release-cutover --host charless-mac-mini` claimed the undeclared user-domain agent without complaint — adoption probes the host first and records what the host reported, and an agent in `~/Library/LaunchAgents` that launchd has loaded is exactly what it is for, so there was no capability gap to close there. `stado service retire` then booted the label out of both per-login spellings and `launchctl disable`d both, which is what makes the retirement survive the next graphical login instead of coming back with it. The host confirmed the postcondition: `no job at gui/501/com.wisent.compute.service.weles-release-cutover`.

One line reverses it, and it is `ensure` rather than `adopt` because adoption alone would re-declare a label launchd is still refusing to load:

```console
stado service ensure com.wisent.compute.service.weles-release-cutover --host charless-mac-mini --from /Users/charles/.stado/bin/weles-release-cutover --reason "restoring the retired release cutover"
```

That is faithful because the unit's argument vector is the program and nothing else, so `ensure` finds the installed plist already declares what it would render, leaves the file alone, and takes its `launchctl enable` + `bootstrap` path — the one branch that undoes a `disable`. Pass no `--arg`: a mismatched vector would make `ensure` rewrite the plist instead of restoring the one that is there.

Proof that a retirement of a writer actually stopped is two reads of the file it was rewriting, far enough apart to span many former cycles. This loop restarted every one to three seconds; `stado service env-show` of `$HOME/.config/weles/worker.env` at 17:56:08Z and 18:02:41Z returned identical assignments, line numbers, values and value states — 6m33s, several hundred former cycles — and the unit's own log gained no line between the two. `stado service list --undeclared` is the other half: the label is still listed, because the plist is still on disk, with no pid and no exit status.

### Getting an unversioned host file back, byte for byte

`env-show` cannot do it, by construction. It replaces every quote, every backslash and every byte outside printable ASCII with `?` and clamps long values, because its job is to let an operator judge a file without a secret crossing the channel. That is the right trade for a configuration reader and the wrong one for a program: `weles-release-cutover` is 4357 bytes whose working parts are a double-quoted `sed -E` program and a line continuation, and an `env-show` transcript of it would not run.

`stado service file-fetch <service> --host <target> --source-file <path> --dest-file <local path>` is the byte-exact counterpart. The host hashes the file itself, the bytes travel base64 inside the same encrypted channel's response, and the digest is recomputed locally over the decoded bytes — two independently computed SHA-256s, because a payload that lost a chunk decodes into something shorter and perfectly valid, so a length can never prove a transfer. A mismatch writes nothing and exits non-zero. `$HOME` confinement and symlink refusal are `env-show`'s prelude word for word, `-L` tested before `-f`; a file past the one-megabyte limit is refused whole rather than truncated, because a prefix hashes consistently at both ends and an operator would commit half a program. Release artifacts are not this command's business: they have published coordinates, a digest and `stado storage`.

`deploy/weles-release-cutover` in this repository is that recovery. Provenance: fetched 2026-08-30 from `charless-mac-mini:/Users/charles/.stado/bin/weles-release-cutover`, 4357 bytes, mode 700, owner-only, SHA-256 `980b734a5015496900959fb535998dc1bfd4b4c8c869088ac9141fe6389191ec` agreed by the host and by this side. It is committed verbatim and deliberately unedited — including the `KeepAlive`-hostile design and the `incident-20260728-gmail8` coordinates it pinned — because its value is as the reviewable record of what ran, not as something to run again. Re-fetching it reproduces the same digest.

### What the loop was pinning, and what converging it actually took

Retiring the writer is what made the file writable; it is not what fixed the host. The loop had been re-imposing `WELES_WORKER_RELEASE_VERSION=incident-20260728-gmail8` on every cycle — a version present nowhere on that host. Its release archive lacks `scripts/worker/deploy/skarbiec-acquisition-scopes.conf`, which is the whole of `verified worker archive is missing its exact Skarbiec acquisition scope catalog`: the message names an archive, not a host, and the catalog it wants ships inside the release.

So `stado host sync-acquisition-scopes` was the wrong instrument, and worth saying why rather than merely not running it. It needs a checked-in catalog source that exists in no repository here, and it registers with `--replace-capabilities`, which replaces the workload's entire Skarbiec capability set and mints a new Ed25519 workload key when the existing one is not one. The catalog it would have delivered was already present, verified, inside the installed release. A command that replaces a working capability set to supply something already in place is not a repair.

The host's own receipts named the answer. `$HOME/.local/state/weles/deployment.release` and `$HOME/.local/share/weles-worker/0.5.21/darwin-arm64/.weles-release` both record `stado://releases/weles-worker/0.5.21/darwin-arm64/weles-worker.tar.gz` with `archive_sha256=316bd651…`, and `$HOME/weles` resolves to that install directory. Two `stado service env-set` writes moved the file onto those coordinates, each confirmed by its own read-back — which is only meaningful because the competing writer was already retired. The host then corroborated the digest itself: the next `auto-deploy.sh` cycle, still holding the old hash, printed `SHA-256 mismatch … expected=0ef1e33a… actual=316bd651…`, hashing the archive on the host and agreeing. That unit had been failing every cycle; it now exits 0.

One key had to come back rather than change. `launch-mac.sh` in release 0.5.21 gates unconditionally on `STADO_RELEASE_API_URL` at line 301 and then never reads it, while `auto-deploy.sh` in the same tree requires it only when `STADO_RELEASE_LOCAL_ROOT` is unset — the correction that exists upstream and not in the shipped launcher. The loop's `sed` had been deleting the key on every cycle, so restoring it is undoing the writer, not patching the launcher: `STADO_RELEASE_API_URL=http://127.0.0.1:8765`, the same Stado surface that serves `/api/release/object` and that `stado host inventory` shows listening under both the `stado-api` and `stado-object` markers on that host. Attribution came before the write: the unit's log carried 182 gate messages and zero `one-time Skarbiec acquisition failed` lines, and under `set -e` a failed acquisition aborts before the gate — so all twelve acquisitions were succeeding and exactly one gated key was absent.

A newer archive, 0.5.22, is staged on that host and was left alone. It is absent from the release store, carries no sidecar digest, no manifest and no provenance, and is not installed. Pointing a production host at unprovenanced bytes no published coordinate attests is the failure the release doctrine exists to prevent, so the convergence stopped at the newest release the host has actually verified and installed.

### A dead unit reported as running, and who owns the worker

`service show` said `runs` whenever the unit FILE existed. It reads `ProgramArguments` out of the plist, reaches no process table and asks launchd nothing, so on 2026-08-30 it reported `com.wisent.always-on.weles` as `runs` while both pids the preceding restart had produced were already gone from `ps` and the unit's stderr ended in `EADDRINUSE 127.0.0.1:58101`. Its word is now `declares`, which is what it always meant, and `stado service serving <name> --host <target>` answers the question that was missing: is the DECLARED unit the process on its own port.

Ownership there is decided by launchd label and never by argv, because two units on that host executed an identical argument vector and argv matching would credit the survivor to whichever one was asked about. The pid holding each port is walked up its own parent chain until a pid appears in `launchctl list`, since a launcher script is the job and the server it starts is the child that holds the socket. A label that cannot be read — a system LaunchDaemon is invisible to an unprivileged `launchctl list` — is `unknown`, never "nobody owns it". The ports judged come from the service directory's declared endpoint or from `--port`, and deliberately not from the unit's env file: that file names every endpoint the unit TOUCHES and most of them are ports it calls, so judging `STADO_API_URL` as a port this unit must own reported three healthy dependencies as stolen the first time it was tried. `endpoint-check` remains the command for dependencies.

The two units were not a mistake anyone made. The Weles release deployer creates one of them: `auto-deploy.sh` copies `$INSTALL_DIR/scripts/worker/deploy/com.wisent.$label.plist` into `$HOME/Library/LaunchAgents` and bootstraps it, for `weles-worker`, `weles-api`, `weles-content-worker`, `weles-keyword-planner-api` and `weles-echo-api`. So the registry declared `com.wisent.always-on.weles` as a system LaunchDaemon while the release kept bootstrapping `com.wisent.weles-worker` in the per-login domain, and the two collided on 58101 with the declared one losing and dying.

Stado is the fleet control plane, so the registry now describes what actually runs: `com.wisent.weles-worker` is adopted — with `--host-heuristic always-on` so the declarative placement carries, and its `weles` onboarding metadata re-attached field for field — and `com.wisent.always-on.weles` is retired. The next `auto-deploy.sh` run therefore re-creates a unit Stado already declares instead of a rival. `service serving com.wisent.weles-worker --host charless-mac-mini --port 58101` now answers `serving`, `served_by_unit`, owner declared `true`.

Reversing it is two declarations and a checked restart, because `retire` deliberately keeps the unit file while withdrawing its registry entry:

```console
stado service adopt com.wisent.always-on.weles --host-heuristic always-on
stado service onboarding com.wisent.always-on.weles --host charless-mac-mini --product-id weles --display-name Weles --repository wisent-ai/weles --surfaces web,worker,operator --first-success-fact authorized_browser_workflow_completed
stado service restart com.wisent.always-on.weles --host charless-mac-mini
```

`retire` now handles the unit's real domain instead of assuming a per-login job. A system LaunchDaemon is stopped through the host account credential, then both `system/<label>` and its recovery job are disabled with privileged `launchctl`; a Linux user unit is stopped, disabled, and runtime-masked so an older coordinator cannot revive it from a stale read. `service remove` composes the same fenced retirement with deletion of the exact managed unit file, while `retire` keeps that file for an explicit rollback.

### Repairing a macOS GitHub runner's apphost signatures

Managed runner installation preserves the signatures shipped in GitHub's
checksum-pinned archive. For an already adopted service that directly starts
`runsvc.sh`, the same repair is available through the CLI and the **Services**
inspector's **Repair GitHub runner runtime** action:

```console
stado service repair-runner-runtime actions.runner.wisent-ai-brama.charless-mac-mini-stado-release --host charless-mac-mini --json
```

The repair retains the installed version and registration, verifies the official
archive's SHA-256 and both replacement apphosts, and replaces only those files.
It does not restart the unit: GitHub's existing listener retry loop owns the
next launch. An intact signature returns `runner apphost signatures are intact;
no files changed`. That result describes the files, not a working runner.
Use `service logs` to read the listener's actual failure.

Linux services and units that do not directly launch `runsvc.sh` are refused.
Missing registration, an ambiguous version, a missing release digest, a digest
mismatch, or a failed signature check stops the repair before activation.
The JSON receipt names the target, unit, runner root, output, and
`restarted: false`; Desktop displays the same output or refusal.

### A live process still executing a binary that was replaced underneath it

A launchd unit's process goes on executing the image it started with. Replacing the file the unit declares does not move it, and nothing on this fleet revisited a unit that was missed: `self_update::recycle_replaced_units` cycles units only inside the invocation that replaced their bytes, matches `argv[0]` by string equality, skips its own pid, defers any unit whose argv carries `agent`, and logs a failed `kickstart` without ever coming back to it. `com.wisent.compute.disk-cleanup.disk-cleanup` recorded `policy:ValueError` 8,348 times across thirteen days from a `--watch` process alive since 27 August, executing an inode its declared path no longer held; an unrelated restart is what ended it. The condition is not rare and not static — the installed binary went 0.13.50 to 0.14.8 inside one day, and measured hours apart on 2026-09-03 the stale set on `lukasz-macbook` lost `com.wisent.compute.agent.lukasz-macbook` to an unrelated restart and gained `com.wisent.stado-resolver` to a new release.

`registry doctor` reports it as `stale-unit-image` when the running and declared files are different inodes, and `unread-unit-image` when the question could not be asked. The identity is `(st_dev, st_ino)` and never a path, because a path is exactly what does not change; `links: 0` distinguishes an unlinked image, where no copy of the running build survives to be diffed, from a replaced one that still exists somewhere. Both readings are local-only: which file a pid executes is answerable only on the machine holding that pid, so every other host gets an explicit unmeasured row rather than a silent pass. A replacement younger than `IMAGE_SETTLE_SECONDS` (300) is an installer mid-flight and is not a finding.

`stado service refresh-image <label>` is the operator verb: it refuses a unit that is not stale and names the identity it found, restarts through `launchctl kickstart -k`, then **re-reads the identity**. That second read is the whole discipline. On 2026-09-03 pid 49727 respawned under `KeepAlive` straight back onto the same unlinked inode it had just left, because launchd re-execs the declared PATH and the path was never the problem, so a restart that did not change the image exits non-zero rather than reporting success.

#### Letting the release agent do it, one unit at a time

The policy is a **top-level registry key**, `release_unit_image_revisit`, and its shape is exact:

```json
{
  "release_unit_image_revisit": {
    "schema_version": 1,
    "targets": {
      "<host>": {
        "state_dir": "/absolute/host/release-state",
        "products": { "<product>": ["com.wisent.example.unit"] }
      }
    }
  }
}
```

Every label listed is one that host's release agent may put back on its declared file, unattended, on its normal tick. **Absent means off**, so a fleet that declares nothing keeps exactly today's behaviour: the pass returns before it reads a process table, a unit file, a lock or a disk. Nothing in this fleet's registry carries the key.

**Why top-level and not a `release_control` field.** Every `release_control` struct carries `deny_unknown_fields`, so a document holding a key an older build does not model is refused *outright* by that build — not ignored. Instance 25 in `checks-that-measure-nothing.md` is the bill for that: `readiness_path` went from forbidden to required with no version where both held, and on 2026-09-01 no single document satisfied the fleet, so the mini's queue agent resolved no policy at all and disk maintenance stopped. A top-level key is not modelled by `Registry`, so it rides in `Registry::extra` and round-trips verbatim through every read and write: older builds preserve it and ignore it, this build reads it. For the same reason there is no `ComputeTarget` field and no declaration-catalog entry — both are modelled surfaces, and adding to them is the same trap in another costume. The typed parser still denies unknown fields *inside* the block: a document may carry keys this build does not know, but a revisit block with a misspelled field is a policy whose author expected something this build will not do, and authorising the part it understood is how a restart nobody asked for gets issued. A block that is present and will not parse is an error, never an empty policy — `stado registry doctor` reports it through `build-refuses-registry`, and the agent says on its own tick why no unit is being repaired.

Seven properties, and each one is a bound rather than a detail:

- **Exact labels, owned per target.** The block authorises the units named in it and never widens; an observation whose label is absent is dropped. It is keyed by target because a launchd label is a fact about one machine — the same product's Linux host runs different units under different names — so a product-level flag would have authorised one platform's labels everywhere, and a bare "this product consents" would have authorised restarting the janitor and the stream writer on behalf of a product with no relationship to either. A `(target, label)` pair claimed by two products is refused when the document is written. The product name is explicit authorization and does not have to appear in `release_control`: Stado's janitor and resolver have no blue-green rollout policy, and the transcript writer is not in that catalogue. Where the shipped product declarations or an adopted service's `onboarding.product_id` positively name an owner, the policy must agree; a `declared_only` onboarding placeholder is not an adopted service and does not count as a runtime ownership witness. Absence from either catalogue does not manufacture a contradiction.
- **Darwin only.** Every target named in the block must have a `release_platform` beginning `darwin-`, because the restart goes through `launchctl`; another platform is refused at the document even when its products map is empty. Left to runtime, every authorised label on a Linux target would fail and record `RestartRefused`. That record bars the identity pair, so it is not a hot loop — but each replacement of the declared file expires the row and buys one more `launchctl` call that cannot succeed for the same reason as the last, so the host spends one futile restart per release indefinitely and records each as a repair considered.
- **At most one unit per reconcile invocation.** One scheduled tick is one invocation, so three stale units require three ticks. The host lock prevents overlapping invocations from racing on the same identity pair; it does not impose a host-wide time or generation rate limit, and a later sequential invocation may act on another eligible unit. A sweep across a fleet agent, a janitor and a stream writer in one invocation is the whole host, and a sweep on this workspace has already turned a degraded host into a down one.
- **Never a unit that recycles itself.** The exclusion is `self_update::defers_to_release_handshake` — the argv carries the `agent` subcommand — reused rather than restated, because the fleet agent is one of the units that goes stale and two spellings of that rule would eventually disagree.
- **The attempt is written before the restart, and a failed one is not retried.** An `Attempting` record is committed to a host-wide ledger in the target's `state_dir` and the restart is refused outright if that write fails, because a record written only after the outcome is lost by any crash in between and the next tick would kickstart the same unit again. The observed result replaces it. A record that did not reach the declared file bars that unit while BOTH identities are still the pair it was made against — so a replaced declared file or a unit something else cycled makes it eligible again, and no wall clock is involved. A surviving `Attempting` record means the pass stopped between recording intent and recording a result, so whether `launchctl` was invoked at all is unknown, and it bars for that reason.
- **One host, one ledger, one lock.** Each target carries one `state_dir`, and the label ownership map is computed from every product in the block before `--product` is applied, so two product-scoped agents share one ledger and cannot each spend a restart on the same unchanged identity pair. A non-blocking lock covers observe, record, restart, settle and record. What it prevents is *overlap*: two reconciles running at the same time would each see the same stale unit against the same unchanged identity pair and each spend a restart, neither having seen the other's ledger write. It is not a rate limit and defines no time window — sequential invocations are separate ticks and each may act on one unit, a different one because the unit already handled is afterwards either on its declared file or barred by its own record.
- **The exclusion is decided from the observation, not a second read.** `observe_unit_image_scan` is the one plist/process/image pass: it matches a unit's whole declared `ProgramArguments` against the live process table and returns an internal enriched row carrying that exact vector beside the stable public `UnitImageObservation`. The public `observe_unit_images` view moves out only the stable observation, while revisit planning keeps the captured argv and reads the subcommand from it rather than re-opening the plist. The decision about whether a unit may be touched therefore comes from the same moment as the pid and image being acted on. Re-reading would introduce the very window this feature exists because of: a replacement landing between the two reads.

What an operator sees stays in the vocabulary they already read. The tick reports the unit, `registry doctor`'s own kind, and the outcome word `service refresh-image` uses; there is no new severity word. The `stale-unit-image` row for an authorised unit gains a clause naming what the agent already tried and what came back — outcome-specific, so a refusal says the identity was not re-read, and a surviving `Attempting` record says only that intent was committed and no result was written, so whether `launchctl` was invoked at all is unknown. A ledger that cannot be read, or a contract that does not resolve, is stated on that row too: the agent will not act, and a doctor that dropped the reason would report the stale unit while omitting why nothing is coming for it. A healthy host emits nothing per tick, and an authorised label the image pass never returned — a typo, or a unit this host never installed — is reported rather than passed over, because "nothing to do" and "nothing found" must not read alike.

### A fleet-wide model outage that was an ungranted entitlement, not a purchase

Every signed agent on charless-mac-mini was refused by Brama with `429 subscription_unavailable: no active stateless provider models for signed agent`, `attempts: 0` — the candidate list was empty before any provider was called. Weles could not run a single browser task because of it.

The refusal is reachable but not readable from outside: `/v1/subscriptions/<agent>` and `/v1/account/subscriptions` both answer 403 once the signature authenticates, because the caller's bearer is bound to a different agent than the signature claims. That 403 is not the boundary it looks like. `broker.rs`'s `list_subscriptions` shells out to an entitlements-router binary on the Brama host, and the ledger it reads is that host's Skarbiec vault: a subscription is a vault item carrying both `brama:subscription` and `brama:agent:<agent>`, with `brama:id:` and `brama:provider:` beside them. `parse_live_subscriptions` reports every live-discovered item as `active`, so "no active subscriptions" means precisely "no item is tagged for this agent" — never "the plan lapsed".

Read where it lives, the answer was unambiguous. The vault held four subscriptions, all `state: active`: codex primary and secondary, claude-code, and kimi. Codex carried `brama:agent:wisent-app` and `brama:agent:lem`; the others carried `wisent-app` alone. Nothing anywhere carried `brama:agent:weles`. The entitlement had never been granted, so there was nothing to renew and nothing to buy.

The repair was `stado host retag-vault-item`, whose own purpose is this: `brama:agent:weles` added to the codex primary and secondary, matching the sharing decision already made for `lem` and giving the worker the fallback `best_subscription_models` is built to walk. A real completion for the signed `weles` identity returned `200` with `ok` immediately afterwards, and the next browser task on that host returned `ok: true` with the page's real title.

Reversing one grant is the same command with the agent tag removed:

```console
stado host retag-vault-item charless-mac-mini provider:codex:brama-sub-wisent-app-codex-primary --tags 'brama:subscription,brama:provider:codex,brama:id:brama-sub-wisent-app-codex-primary,brama:agent:wisent-app,brama:agent:lem'
```

`--tags` is now optional, and omitting it reads. That is not a convenience. The command replaces a tag list rather than adding to it, so an operator who cannot see the list they are replacing must guess it — and a guess that drops `brama:agent:lem` unsubscribes another agent from a paid plan while every check that counts credentials keeps answering green. The read is the same host-side `read_vault_phase` the write already used for its before/after report; it simply stops before writing.

One defect found on the way is left named: the worker's model-router bearer and its agent signature identify two different agents, which is why the account-scoped reads answer 403 for a caller whose signature is valid. The completion path resolves subscriptions for the SIGNED agent, so this did not cause the outage, and the grant above fixed the outage without touching it. It should still be reconciled at the source — the credential the launcher acquires from Skarbiec — because an identity that is two identities will mislead the next person who reads it.

## Service directory

The directory joins a producer, its endpoint, and declared consumers. `stado service directory show <service>` displays the resolved relationship. `stado service directory connect <service> --consumer <consumer>` establishes a declared connection. `stado service verify <service>` exercises reachability from the declared consumers instead of treating the producer's loopback listener as fleet reachability.

A directory declaration is not proof of a live route. Verification records which consumer reached which endpoint and why a refusal occurred.

### The marker holds the address that host dials

Several products resolve a service from an owner-only file, `~/.stado/forwards/<service>.local`, rather than from an environment variable: Skarbiec's credential bridge reads `weles-admission.local` this way. `stado service directory publish` writes those files, and what it must write depends on where the service runs.

- The host that SERVES the service gets the address it serves on, from `endpoints[<that host>]`.
- Every other host gets its OWN resolver adapter for that service, from `service_resolver.adapters[]`, because the serving host's loopback port means something else — or nothing — on their machine.
- A service whose resolver declares one adapter per consumer is refused by name, listing the consumers: the marker's filename carries no consumer, so nothing elects one consumer's socket for the rest.

`publish` reports the source of every address it wrote (`directory-endpoint` or `resolver-adapter`), reports every marker no declaration accounts for as a fossil, and removes exactly those under `--prune`. `stado host inventory <host>` judges each marker against both declared sources and prints `declared_source` beside its verdict, and Stado Desktop's Hosts screen shows the same rows under **Service addresses this host dials**.

Publishing skipped every service placed elsewhere until 2026-09-05, which left those markers as whatever last wrote them: `lukasz-macbook` carried `brama.local` at `127.0.0.1:8080`, Brama's port on the Mac mini and an unrelated service's port on the laptop, and `weles-admission.local` at `8788` while that host's adapter binds `17614`. A consumer reading either file dialled the wrong service, and the inventory called the correct address `undeclared` because it compared markers with `endpoints` alone.

## Object authorization

The object API authorizes an action in two stages:

1. the caller bearer selects one namespace policy;
2. the object API verifier uses its own host-local Skarbiec grant to read the exact namespace credential items declared in `object_api.namespaces`.

The `probierz` policy must grant every prefix in
`queue::copy::CANONICAL_PREFIXES` for get, put, list, stat and delete, because
that list is what the queue reads and writes. `stado config validate`, `config
set` and `host config-set` refuse a policy that leaves one out, naming it, and
`stado doctor` on the host fails `object-auth` with the same sentence. The
rule exists because `job-transitions/` arrived in the binary on 2026-09-01
with no grant on any host: the object API answered every agent claim with 401
and the agent restarted after each one until 2026-09-03, while its capacity
broadcast kept saying the host was alive.

The verifier bearer lives in `WC_OBJECT_SKARBIEC_TOKEN_FILE` on the object API host. To restore its grant without moving the bearer off that host:

```console
stado host reconcile-object-verifier <target> --json
```

The command derives the item set from `object_api.namespaces`, asks Skarbiec on the target to bind the existing bearer to that exact set, and reports item names and expiry only. It never prints the bearer.

Release publication has a separate verifier and per-product policy set. Reconcile
the verifier to the complete declared publisher set:

```console
stado host reconcile-release-verifier <target> --json
```

The command compares the caller's `release_api.publishers` with the configuration
the target actually consumes and refuses before mutation when they differ. It
copies every controller-owned publisher item into the target-local shadows and
binds the existing verifier bearer to exactly their `token` reads, removing
capabilities for retired publishers without rotating or printing the bearer.
Release preflight proves the caller credential with an authenticated operation
under the same product prefix. A public `releases/` stat proves neither boundary.

Use `stado storage stat <stado-uri> --json` as the smallest final check. `present` and `absent` are both authoritative answers. `503 object authorization unavailable` means the verifier boundary failed; it is not evidence that the requested object is absent.

### The object API runs the managed binary, since 2026-09-04

`com.wisent.always-on.stado-object-api` used to execute a private service
tree, `.../services/com.wisent.always-on.stado-object-api/current/$STADO_PLATFORM/stado`,
which nothing in the release pipeline ever moved: the control-plane job
delivers with `stado host declare-version` plus `stado service converge
<host> stado --apply`, and that pair resolves one root per managed binary,
`$HOME/.stado/bin/<binary>`. The object API was not on that root, so it was
the one unit on the host frozen at whatever build last installed it by hand
— on 2026-09-04, a build old enough that its release refusals carried no
`reason` code, while every other unit on the host had rolled forward many
times.

It now runs `$HOME/.stado/bin/stado`, the same managed binary as the
resolver, the control plane, the release agent and the queue agent, and it is
listed among the `stado` product's units. So `stado host declare-version
<host> --binary stado --version <v>` followed by `stado host release <host>
--binary stado --version <v>` moves the object API with every roll and
restarts it with the rest, and the `deploy-control-plane` job needs no step
of its own for it.

The corollary is worth stating because it caused an outage before it was
understood: a unit's program and the archive a version arrives in must
agree. `stado service update --from-archive` now reads the unit's declared
program, inspects the archive's member list, and refuses when the program is
not in it, naming both — `the unit runs current/darwin-arm/stado; the
archive holds bin/stado`. Relinking `current` at a tree without the
program does not fail at install time. It fails at launchd's next spawn,
which cannot say why, and a KeepAlive job that cannot spawn leaves its
domain, so the repair stops being a rollback and becomes a privileged
bootstrap.

## Workload grants and service authentication

`stado service grant-sync` binds an existing owner-only token file on one host to an exact consumer and capability set. Skarbiec reads the bearer locally and stores only its digest:

```console
stado service grant-sync <service> \
  --host <target> \
  --consumer <consumer> \
  --capabilities '<item>#<field>:read' \
  --token-file '$HOME/.stado/<consumer>-skarbiec-token' \
  --json
```

`stado service auth-check` then sends that bearer from the host to a read-only loopback endpoint. With `--repair`, it synchronizes the named item field into the managed environment, restarts only the declared unit, and checks the endpoint again. `--take-over-listener` is a separate, explicit recovery for an unmanaged process occupying the declared port.

## Items in a host's vault

`stado host vault-item-put <target> <item> --type <kind>` stores one canonical item, reading the payload from stdin so no credential field enters a local or remote argument vector. `stado host vault-item-show <target> <item>` is its read: kind, schema, revision, tags, `updated_at`, and per field the name, byte length and SHA-256, narrowed with `--field <name>`. The decryption and the hashing both happen on the host, so comparing a digest against a local copy's answers "does the host hold what this declaration references" without either side sending the value.

The read exists because its absence hid a whole migration's work in the wrong place. `skarbiec set-json` on a workstation writes that workstation's vault; the fleet reads the target's own live vault, and nothing pointed at the difference — `retag-vault-item` reports state, revision and tags but nothing about a payload, `stado credentials get` reads the local store, and `skarbiec get` is not a host-exec command. Seven environment bundles and twenty credential fields went into a laptop vault nothing on the fleet reads, and the only symptom was Brama answering `401` to a bearer it had never been told about.

## Failure ownership

| Result | Meaning | Repair owner |
|---|---|---|
| target absent from registry | the host channel has no declared destination | registry declaration |
| host identity mismatch | transport reached a different machine | host enrollment |
| directory has no consumer relationship | route was never declared | service directory |
| `401` or `403` from final endpoint | caller grant or policy rejected the operation | caller/workload grant |
| `503 object authorization unavailable` | object API verifier cannot read namespace credentials | object verifier reconciliation |
| endpoint answers but final state is unchanged | process health passed, product operation failed | owning product workflow |

Retries do not repair a declaration or grant. They are appropriate only after a retryable transport failure where the same declared operation remains valid.

## Tests

Repository tests live under `stado-rs/tests/<area>/main.rs` and drive the real
`stado` binary. The directory listing is the inventory; this page does not copy
it, because a hand-copied list rots into names that do not exist. The journeys
those tests answer for are declared in the Probierz manifest for app `stado`.

The channel boundaries above are defended by:

- `tests/channel/` — the public release channel: stat, download, digest verification, and execution of one immutable native release through the public HTTPS origin;
- `tests/service/` — managed service grant synchronization, authentication checks, and service transitions;
- `tests/recovery/` and `tests/recovery_release/` — host-channel recovery and signed release recovery;
- `tests/domain/`, `tests/link/`, `tests/removefile/` — host-channel identity, declaration, and guarded mutation.

Probierz is the execution and evidence boundary when it is operational. The test source remains in this repository. A passing parser, dry run, mock server, or successful process start is not channel evidence; the journey must observe the promised final state in the real connected component.

## When the stable bind is gone

A Wisent product on a Darwin host serves on two ports, not one.
`release_control.products.<product>.targets.<host>` declares a `stable_bind`
and a pair of `candidate_ports`: for Skarbiec on `charless-mac-mini` those are
`127.0.0.1:8895` and `[18895, 18896]`, and for Brama `127.0.0.1:8080` and
`[18080, 18081]`. Every consumer's configuration names the stable bind and
nothing else. The candidate is where the release itself listens, and the
stable bind is a proxy held by the release agent, which is what makes a
blue-green rollout invisible to the callers: the agent brings a candidate up,
probes its `readiness_path`, and moves the stable proxy over.

**The release agent is the only thing that publishes a stable bind.** The
legacy launchd daemon named by `legacy_launchd_label` does not: in a settled
blue-green state that daemon *is* the live candidate, so restarting it moves
nothing and only interrupts the running service. `stado host recover` reports
`candidate_live:<port>` for exactly that case and touches nothing.

### Why a namespace declared without its grant takes the stable binds down

The agent learns which ports to publish from `release_control` in the
canonical registry, which it reads through the object API. The object API
gates every non-release object read on its object authorization boundary, and
that boundary is open only while the host's object verifier holds a read on
the Skarbiec item of **every** namespace in `object_api.namespaces`. One
namespace declared without its item in the grant closes the whole boundary,
and the host's log says so:

```text
object authorization boundary revalidation failed: Skarbiec deployment configuration:
object verifier grant item set mismatch (missing=[spis-crawls-object-api], unexpected=[])
```

Nothing fails at that moment, which is the trap. Existing processes keep
serving from cached tokens and the last-known-good registry. The bill arrives
at the next restart of anything that reads the registry — and a version roll
restarts the release agent. The agent then cannot read `release_control`,
publishes no stable bind, and the ports every consumer names go quiet. On
2026-09-03 that sequence left `https://brama.wisent.com/health` answering 502
for hours while two `stado host release` runs reported `ok` with every step
`ok`, because the object API needs Skarbiec's stable bind to open the very
boundary that was closed.

### The repair

Read the boundary's own reason first, because it names the item:

```console
stado host unit-log <host> com.wisent.always-on.stado-object-api --lines 300 | grep 'object authorization boundary'
```

Then declare the missing namespace on the machine you are running from —
`reconcile-object-verifier` computes the item set from the local
configuration, so a namespace that exists only on the host can never be
satisfied from elsewhere — and reconcile the host's grant:

```console
stado config set object_api.namespaces.<ns> '<the same JSON the host declares>'
stado host reconcile-object-verifier <host> --json
```

`exact: true` with the item in the list is the answer. The boundary opens, the
release agent starts on its next tick, and the stable binds come back on their
own. Verify in this order:

```console
stado storage stat stado://<queue-namespace>/registry.json      # state present, no 503
stado host exec <host> -- lsof -nP -iTCP -sTCP:LISTEN           # every stable bind held
stado host unit-log <host> com.wisent.stado.release-agent       # no infra_down loop
stado service verify --host <host>
```

`stado host config-set` warns at declaration time when a namespace names an
item the local configuration does not cover, so this does not have to be
learned twice.

### The fallback, and its reversal

When the boundary cannot be opened quickly and a serving port must come back
now, point the host's verifiers at the product's **declared candidate** —
which is a legitimate blue-green address, not an invented one — and reverse it
in the same session:

```console
stado host config-set <host> release_api.skarbiec.url http://127.0.0.1:18895
stado host config-set <host> service_api.skarbiec.url http://127.0.0.1:18895
stado host config-set <host> secrets.skarbiec.url     http://127.0.0.1:18895
stado host config-set <host> object_api.skarbiec.url  http://127.0.0.1:18895 \
  --reload-service com.wisent.always-on.stado-object-api
```

One reload covers the release, object and service verifiers: `stado dashboard`
is the single process serving `/api/object`, `/api/release/object` and
`/api/service/*`. `secrets.skarbiec.url` belongs to the control plane and
takes `--reload-service com.wisent.compute.service.stado-local-control-plane`;
restarting it is safe only while Skarbiec is answering at the address you just
set, which is the point of setting it first. Reverse every one of the four to
the stable bind once the agent has published it again, with the same reloads,
and verify with the same four commands.

### What now prevents the recurrence

- The release agent falls back to this host's last-known-good `release_control`
  when the authority cannot be read, the way the resolver already did for
  `service_directory`, and says `release agent recovery: …` when it does. A
  closed boundary no longer takes the stable binds with it.
- `stado host release` polls every declared stable bind of the units it
  restarted, for up to 120 seconds, and reports `stable_binds` with a verdict
  per port. It refuses to report `ok` while one is `absent`, so a roll can no
  longer succeed on paper through an outage.
- `stado host recover` reports every declared stable bind as `already_bound`,
  `candidate_live:<port>` or `refused:<reason>`, and bootstraps only a
  declared bind that nothing at all is holding.
