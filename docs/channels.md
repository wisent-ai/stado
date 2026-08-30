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

`host exec`'s allowlist takes no operator-supplied path, so it reads no files. A managed unit's owner-controlled env file — the one a launcher `.`-sources, not the one the unit file declares — is read with `stado service env-show <service> --host <target> --env-file <path>`, through the same channel and the same `$HOME` confinement `stado service env-set` writes through. Values whose key looks like a credential, and URLs carrying userinfo, are withheld on the target and never cross the channel; endpoints, ports and variable references are shown, because those are what an operator must verify. `stado service endpoint-check` reconciles the loopback endpoints that file declares against the target's own socket table and exits non-zero when a declared dependency is dead.

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

## Service directory

The directory joins a producer, its endpoint, and declared consumers. `stado service directory show <service>` displays the resolved relationship. `stado service directory connect <service> --consumer <consumer>` establishes a declared connection. `stado service verify <service>` exercises reachability from the declared consumers instead of treating the producer's loopback listener as fleet reachability.

A directory declaration is not proof of a live route. Verification records which consumer reached which endpoint and why a refusal occurred.

## Object authorization

The object API authorizes an action in two stages:

1. the caller bearer selects one namespace policy;
2. the object API verifier uses its own host-local Skarbiec grant to read the exact namespace credential items declared in `object_api.namespaces`.

The verifier bearer lives in `WC_OBJECT_SKARBIEC_TOKEN_FILE` on the object API host. To restore its grant without moving the bearer off that host:

```console
stado host reconcile-object-verifier <target> --json
```

The command derives the item set from `object_api.namespaces`, asks Skarbiec on the target to bind the existing bearer to that exact set, and reports item names and expiry only. It never prints the bearer.

Release publication has a separate verifier and per-product policy set. Reconcile
only the product being published:

```console
stado host reconcile-release-verifier <target> --product <product> --json
```

The command resolves `<product>` through `release_api.publishers`, reads only that
controller-owned publisher item, writes only its target-local shadow, and adds
only its `token` read to the existing verifier grant. Other product capabilities
and shadows are preserved and are never rotated by this operation. Release
preflight proves the caller credential with an authenticated operation under the
same product prefix. A public `releases/` stat proves neither boundary.

Use `stado storage stat <stado-uri> --json` as the smallest final check. `present` and `absent` are both authoritative answers. `503 object authorization unavailable` means the verifier boundary failed; it is not evidence that the requested object is absent.

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
