# Operational model

How a running Stado is configured, what it stores where, how work is placed
and retried, and what it reports when something is wrong.


## Configuration

`STADO_CONFIG` selects the deployment profile. The minimal local profile has no
cloud or product-specific credentials. Production profiles define provider
order, disabled providers, storage, deployment identity, API verifiers,
ownership, and policy. Environment variables are limited to documented
route-local overrides.

See [Configuration and credentials](https://stado.wisent.com/docs/configuration).

## State and ownership

One configured backend is canonical. Client reads may consult a compatible
backup only after a primary error; a successful absent answer from the primary
is authoritative. Destructive janitor reads and object API server reads stay on
the primary. Mutations commit there first and are then mirrored best-effort to
the compatible backup, which is never promoted to writer. Queue migrations use
pause, drain, copy, verification, fencing, and explicit cutover.

To distinguish an authority switch from deletion without reading object
content or walking either store, compare an existing coordinate in the fixed
host roots:

```console
stado host backup-audit TARGET --object stado://probierz/queue/JOB.json --json
```

Repeat `--object` to compare more coordinates. Exact-object mode reports each
side's state, byte count, SHA-256, and effective metadata identity, and reports
`deadline_unproven` if the existing read-only hashing budget expires.
`--inventory-namespace NAME` instead lists backup-visible paths and size
metadata in an explicitly selected namespace without reading object bodies.
Neither mode can be combined with replica reclamation. Omitting both retains
the whole-replica classification.
Even exact equality at selected physical coordinates does not identify which
root the running API serves; that requires the loaded service environment and
process identity reported by `service label-print`.

The command owns one durable transaction rather than using `object-relocate`
(which is an in-store move):

```console
stado host storage-root-reconcile TARGET --transaction ID --phase run --json
stado host storage-root-reconcile TARGET --transaction ID --phase resume --json
stado host storage-root-reconcile TARGET --transaction ID --phase status --json
stado host storage-root-reconcile TARGET --transaction ID --phase rollback --json
stado host storage-root-reconcile TARGET --transaction ID --phase finalize --json
```

Run starts the transaction; Resume continues that same ID after interruption;
Status is read-only. Run, Resume, Rollback and Finalize launch a target-resident,
globally locked native worker and first report `accepted` once that worker owns
the request. Acceptance is not completion, and neither the CLI, API nor Desktop
automatically chooses a follow-up phase. Read Status explicitly after every
accepted action.

Run and Resume own the internal checkpoint, apply and activation stages. They
capture the queue and each writer's exact native state, resolve and stage the
target's current declared Stado runtime before the fence, acquire placement
leases, pause and drain the queue, stop storage writers while retaining the
transport and current runner, and hold the storage-write fence. Only then do
they take verified immutable snapshots of both complete physical roots.

The object API's captured loaded route fixes the transaction's authority. If A
was serving, A's body and metadata win shared-path conflicts and only B-only
objects are imported. If B was serving, B wins shared paths while A-only
objects remain. The qualified `ecosystem/` namespace and its matching metadata
are copied additively from the immutable B checkpoint into A; B is never
changed, and the handoff is not an unconditional B-wins overwrite.

Rollback is resumable only before the data-commit boundary. It restores A to
its exact physical checkpoint, removes transaction-created imports and empty
directories, and restores the captured route, services and queue state. After
data commit, Resume the same transaction to finish the recorded activation
instead of rolling back or starting another transaction.

Run or Resume continues automatically through typed lifecycle classification,
verified additive application, activation of the target's declared runtime,
exact native-service and queue restoration, and write-fence release. It then
records `activated_pending_lifecycle`. Let the ordinary coordinator complete
the typed cleanup of queued cancellations and retained outcomes, then
explicitly run Finalize and use a later Status receipt to prove `complete`. The
full physical snapshots remain the transaction's fenced evidence; finalization
records successful lifecycle cleanup rather than deleting that proof.

Provider resources are mutable only when their ownership and expected state
match the approved plan. Report-only is the default autonomy level.

The canonical registry is one document, and a write replaces all of it, so
every write is conditional on the generation it was read at.
`stado registry pull --with-generation` hands back the document and that
generation together from one read (`--generation-only` prints just the token),
and `stado registry push --if-generation <token>` refuses the write unless the
canonical object is still at that generation. A refused write exits `75` and,
with `--json`, prints a `stado.registry-push-receipt.v1` object whose state is
`conflict` and which names both the expected and the actual generation; a
storage or validation failure keeps exit `1` and prints no receipt, so the two
are never confused. A reconcile loop answers `75` by re-reading, re-applying
its change to what the registry now says, and pushing again with the new
token — never by `--force`, which waves past the deleted-key guard and has no
bearing on a generation that has moved on.

## Credentials

Local onboarding requires none. Production callers and adapters use separate,
least-privilege identities. Workload secret references name an item and field;
plaintext is resolved only at execution time.

## Upgrades and rollback

Operators pin an exact immutable version and platform. Upgrade requires a
verified release manifest, compatible schema range, backup, health check, and
rollback coordinate. No runtime follows a mutable `latest` binary.

One version means one build, and that is enforced where publication starts
rather than where delivery ends. Two publishers write disjoint objects under
`releases/<product>/<version>/<platform>/`, so object-level immutability alone
cannot keep their commits coherent. Every
`stado release claim-coordinate PRODUCT VERSION PLATFORM --source-commit COMMIT`
first creates or confirms the shared platformless claim at
`releases/<product>/<version>/source-revision.json`, then mirrors that identity
at the platform coordinate for compatibility. The version claim is one
create-only arbitration point even when publishers start concurrently or the
platform set changes. A second commit is refused before artifact bytes are
written; an older platform-only version can be backfilled only after every
existing platform claim is valid and agrees.

Promotion and delivery compare signed and compatibility manifests with that
shared claim. Coordinates published before version claims remain readable
through the validated platform claim, but no missing or malformed claim is
silently treated as agreement.

Stado release artifacts contain the client only; they never bundle or apply
Supabase migrations. Database rollout is versioned and deployed from
[`wisent-supabase-oko`](https://github.com/wisent-ai/wisent-supabase-oko)
before a Stado release consumes the changed contract.

See [Release and compatibility](https://stado.wisent.com/docs/release) and
[Operations](https://stado.wisent.com/docs/operations) for release and recovery procedures.

## Recover the Stado binary on a host

Use the versioned recovery path only when the target's Stado binary or resolver
is missing or broken badly enough that routine delivery cannot run. The
canonical operation is one command:

```sh
stado host recover TARGET --release 0.7.34
```

The command reads the last valid canonical registry snapshot (or the bundled
snapshot when `--bundled-registry` is explicitly supplied), restores the
registry-selected release object API before its first catalog read, and
downloads the exact canonical signed artifact without using the local resolver
or a remote Stado binary. It verifies the release manifest, signature, and
SHA-256 before activation. It preserves the previous binary, installs by
atomic replacement, and probes the new binary remotely with
`stado resolver --help`. A failed probe
atomically restores that backup (or removes the invalid install when no prior
binary existed); only a successful probe continues into the existing host
recovery. Do not split those stages into manual copy or SSH steps.

`stado host release TARGET --binary stado --version VERSION` remains the normal
declaration-driven delivery path for a healthy fleet. `host recover --release`
is the break-glass bootstrap for restoring Stado itself when the resolver or
remote binary that normal delivery depends on is unavailable. Leaving
`--release` out preserves the original recovery behavior.

For Stado's native delivery, `release install-local` and
`service converge --apply` share the verified archive retained under
`$HOME/.stado/releases/stado/<version>/<platform>/`. Root delivery moves its
already-downloaded archive there before activation; it does not restart
registry units whose executable lives in an independently installed
`$HOME/.stado/services/...` tree. Reader convergence then uses the existing
idempotent `service update --from-archive --refresh-image` path for every such
registry declaration. A resumed pass with an already-attested root fetches the
exact archive only when the retained copy is absent or corrupt, leaves
already-current root and reader images running, and fails unless every stale
private image is installed and proved. The queue agent continues to defer its
own recycle through the installed-release handshake.

On a required-delivery retry, `install-local` compares the already-verified
payload with the installed root. Byte-identical root bytes are not renamed;
the endpoint repairs their attestation and release-version handshake, checks
every global reader's live image, then starts private updates through the explicit installed
`$HOME/.stado/bin/stado` path. Failed child JSON, stdout, and stderr remain in
the `stado-readers.detail` receipt. The Desktop Services action runs the same
host-wide or selected-binary CLI apply, preserves its decoded report plus
actual exit status, and never re-derives the gate.

Partial-state reader resume requires the target's global receiver to come from
a release containing the retained-archive arguments. A version banner alone
does not establish that source identity. A receiver built without the hidden
contract rejects the apply and leaves convergence failed; first deliver a
release that contains this fix through the normal root path. There is no
compatibility shim and no success inferred from an older source.

The repository's `.github/workflows/deploy.yml` invokes that CLI convergence
for the existing stable deployment workflow; it does not define a second
private-reader loop and is not the canonical source, qualification,
publication, or promotion contract. Those remain owned by
`stado release submit`; GitHub is an optional adapter.

## Observability and recovery

`stado overview`, `stado doctor`, queue state, heartbeats, leases, provider
inventory, billing signals, and result manifests provide evidence. An
unreachable store must be reported as unreachable, not as an empty queue.

Incident and recovery procedures are published separately from product documentation.
See [Operations](https://stado.wisent.com/docs/operations).

