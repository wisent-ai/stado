# Disaster recovery

A dependency is down — a provider, a billing account, a storage backend, a
host — and you need to know what is affected, move what must move, and keep a
truthful record of what happened. This page is the order an operator acts in.
Individual flags live in the [cli](cli.md) reference; day-two procedures in
[runbook](runbook.md).

## 1. Inventory before you touch anything

`stado blast-radius` is a side-effect-free assessment of one failed
dependency:

```bash
stado blast-radius --dependency gcp --json
```

It reports the dependency's live resources, auth, consumers, storage, and DR
coverage, and it deliberately keeps those failure domains separate instead of
collapsing them into "the queue is empty": primary and backup stores, Skarbiec
credentials, live cloud resources and caller/runtime IAM, downstream
consumers, and backup namespace coverage each get their own answer. Provider
probes are independent and paginated, so one disabled API cannot hide the
remaining project inventory.

The command never selects a backup for you. Queue state contains CAS locks,
leases, and moving job records; a transparent read fallback can make two
schedulers dispatch the same work from divergent stores. Promotion must fence
writers first, then select one backend for every participant — which is the
next step.

## 2. Transactional recovery: fence, migrate, verify, cut over

`stado recovery migrate` is the fenced, provider-neutral cutover: drain, copy,
verify, cut over selected services, and optionally resume.

```bash
stado recovery migrate \
  --from gcs --from-bucket wisent-queue \
  --to local --to-path ~/.stado/local-storage \
  --writer control-host:stado-coordinator \
  --activate control-host:stado-coordinator \
  --enable-provider local \
  --dry-run
```

Drop `--dry-run` to execute; with it the command validates and prints the
plan, performing no network or filesystem writes and no billing change.

What fencing guarantees:

- Every source writer named with `--writer HOST:SERVICE` is stopped before
  anything is copied. Omitting `--writer` requires `--source-offline`, an
  explicit assertion that no unlisted source writer can run — including
  schedulers, Cloud Functions, Cloud Run jobs, coordinators, monitors, and
  agents.
- Every service named with `--activate` is fenced before it is restarted on
  the destination.
- The queue stays paused at every failure boundary. Without `--resume`, the
  destination stays paused even after a fully successful cutover; resuming
  dispatch and claims is a separate, explicit decision.
- Only explicitly named services and compute providers are switched.
  `--enable-provider` is the complete post-cutover allowlist, and `gcp` is
  rejected in it.
- An optional GCP billing window (`--manage-gcp-billing`) is opened only
  around source fencing, copy, and verification, and closed before any
  workload is resumed. `--confirm-billing-window` must exactly repeat
  `--gcp-project` before a billable API call is made.

What refuses to proceed: the same fenced-transaction discipline as
`stado placement move`, whose contract is the registry placement profile —
concrete units per host, stop/start order, durable files, loopback health
probes, and routing units. The command claims the profile through registry
CAS, fences the source, copies state only after writers stop, activates and
probes the destination, then commits the service declarations with a second
CAS. Every failure before that commit restores destination files, routing,
and source services. In particular, a required state file that is present when
the move is planned but missing when read after writers stop fails the
transaction with `required state <path> disappeared after fencing` — the
world changed under the transaction, and the command refuses to cut over
without the state rather than committing a move that silently lost it. A
transaction record that vanishes before the second CAS fails the same way:
`placement transaction disappeared before commit`.

## 3. Queue-state migration between backends

`stado storage` moves queue state between storage backends — the
billing-outage migration path when the control plane must leave a provider:

```bash
stado storage copy --from gcs --from-bucket wisent-queue \
  --to local --to-path ~/.stado/local-storage --dry-run
```

Omitting `--prefix` copies the whole canonical prefix set. Verification is a
separate read-only pass that compares two stores object-for-object and copies
nothing:

```bash
stado storage verify --from gcs --from-bucket wisent-queue \
  --to local --to-path ~/.stado/local-storage
```

`stado storage backup` copies the active queue store to the configured
disaster-recovery store; `ls`, `stat`, and `cat` inspect objects without
writing. `recovery migrate` composes these with fencing and cutover; use bare
`storage copy` only when you have established by other means that no writer
can run.

## 4. What the backup store is — and is not

The configured backup backend is a read fallback for the same deployment, not
cross-provider disaster recovery. The config validator states the contract
directly: the replica is read fallback only and is never promoted
automatically. `stado doctor` says the same about the default layout — a local
backup path mirroring the local primary is temporary same-disk protection
only — and the [2026-07-27 GCP billing outage
report](incidents/2026-07-27-gcp-billing-outage.md) records the operator
config carrying that exact warning. Cross-provider DR is an explicit,
fenced migration (steps 2–3), never an automatic failover. See
[configuration](configuration.md) for the storage layout.

## 5. The durable account of the outage

A gap in a host's beacons closes over itself the moment the host returns: the
beacon prefix holds only the latest document per host, so after the 2026-08-19
`control-host` tailnet drop the product could not say the six-minute outage
had happened, and the resolver's true, timestamped refusals existed only in a
local log file read by nobody. Two append-only blob families now keep the
record:

- `state/host_silence/<host>/<started_at>.json` — one record per gap, opened
  when the newest beacon crosses the silence threshold and closed by the first
  fresher beacon. `started_at` is the last moment the host was heard from, not
  the moment somebody noticed, so the duration measures the outage rather than
  the polling interval.
- `state/reader_refusals/<host>/<at>.json` — one record per refusal, carrying
  the refusing component's own sentence verbatim, so the operator greps for a
  string that exists in a source file. `<host>` is the subject of the refusal,
  not the machine that refused: a laptop resolver failing to reach the
  authority on the Mac mini is evidence about the Mac mini.

These records are where a post-incident account of "what was unreachable, and
for how long" comes from, and they live under `state/` because that prefix is
on the object gateway's allowlist — the first cut wrote elsewhere and every
write was silently refused while stale reads kept flowing (see
[operations](operations.md)).

## The order rule

Three rules from this fleet's own incidents, in force everywhere above:

- **Read the unit before you cycle it.** `stado service show` prints what a
  managed unit actually runs — its program, arguments, and unit file — and
  repair signals only the pids a probe actually found; nothing re-derives a
  target from a pattern. A loaded unit must prove its live process matches the
  declared program before Stado adopts or restarts it.
- **Prefer in-place restart.** `service restart` and `service ensure` kick a
  loaded job in place and never unload it, so there is no window in which the
  job does not exist. The old bootout-then-bootstrap order could fail with the
  unit left unloaded — a partial failure strictly worse than never having run
  the restart — and did, on the always-on host.
- **A failed repair is a reason to stop.** Daemon restarts send TERM only,
  with no escalation: a control-plane daemon that ignores TERM is a finding to
  report, not a reason to try SIGKILL on the process holding the fleet's
  authorization state. A failed kick reports `restart_failed` with the exact
  privileged command an operator could run, and stops. Autonomy does the same:
  unproven ownership, an unavailable probe, and failed repair produce durable
  reconciliation records and alerts, never a guessed deployment.
