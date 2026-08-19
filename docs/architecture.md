# Architecture

The high-level plan — Stado laid out against the product-guidelines
reviewable sequence, with completion gates per stage — is
[architecture-plan.md](architecture-plan.md). This file is the mechanism
reference below it.

## Data flow

```text
submitter
    |
    v
authenticated Stado machine / object boundary
    |
    v
configured canonical queue backend
    |
    +----> Rust coordinator ----> enabled provider adapter ----> Rust agent
    |                                                        |
    +--------------------------------------------------------+
                                                             |
                                                             v
                                      canonical status and output objects
```

The selected `STADO_CONFIG` owns provider order, explicit fencing, primary
storage, replication, release origin, and credential-verifier boundaries.
Coordinator and agents use the same canonical object prefixes regardless of
backend. There is no Cloud Function scheduler, provider-derived storage
fallback, or direct client bucket path.

## Logical service resolution

Workloads address `stado://service/<name>`, never a host, tailnet address, or
service port. Every registered host runs a loopback-only Stado resolver. New
clients use its resolution API; clients that still require an HTTP origin use
a stable local adapter owned by the same resolver.

```text
workload -> 127.0.0.1:stable-port -> local Stado resolver
                                      |
                                      +-> authority snapshot over registry SSH
                                      +-> consumer capability policy
                                      +-> active placement host
                                      +-> direct loopback or registry SSH transport
```

`service_directory.authority` names the one target and Stado binary allowed to
serve canonical snapshots and commit routing changes. Every other resolver
fetches that versioned snapshot over the authority target's registry-owned SSH
transport; it never treats its bootstrap registry copy as current routing.
`service_directory.generation` is the routing epoch. `placement move` delegates
to the authority, then updates the active host for every service in the
placement group and increments that epoch in the same compare-and-swapped
registry commit that moves the service declarations. While the transaction lock
exists, resolution for that profile fails closed. Resolver caches reject
generation rollback and stop accepting connections after `max_stale_seconds`
without a successful authority refresh.

Physical endpoint URLs are host-relative loopback origins. A resolver on the
active host connects directly; another host uses only the active target's
registry-declared SSH transport. Neither form is returned by the resolution
API. Skarbiec remains the authority for narrow credentials, while Stado owns
service discovery, workload-to-service capability admission, and transport.

## Signed product release flow

```text
tagged source -> build each platform once -> qualification evidence
      -> signed immutable Stado coordinate -> desired registry generation
      -> host release agent -> private candidate port -> readiness
      -> stable loopback proxy cutover -> drain -> rollback window -> commit
```

Promotion changes references, never bytes. The signed manifest binds product,
SemVer, platform, source revision, archive digest and size, binary/launcher
paths, config/state schemas, minimum Stado version, rollback compatibility,
qualification evidence, builder and key id. The host re-verifies every binding
before extraction and rejects links, traversal, excessive entry counts or
expanded size.

Cutover intent is persisted before the proxy target changes, so reconciliation
can finish an interrupted transition. The prior process remains live on its
private port through the rollback window. Lost readiness, a failed proxy, or a
failed drain returns routing to that process; a first migration restores the
legacy launchd service. The failed artifact digest is quarantined per host, so
the same desired generation cannot restart-loop. Desired, observed and
quarantine state contain no provider or product secret material.

## Canonical object layout

Job state lives in the backend selected by `STADO_CONFIG`:

| Prefix | Contents |
|---|---|
| `queue/<id>.json` | Pending jobs and submission ledger. |
| `running/<id>.json` | In-flight jobs with owner, instance reference, and start time. |
| `completed/<id>.json` | Successful terminal jobs. |
| `failed/<id>.json` | Unsuccessful terminal jobs with bounded error classification. |
| `cancelled/<id>.json` | Durable operator cancellation record. |
| `status/<id>/...` | Heartbeats, redacted output, result manifest, and artifact evidence. |
| `leases/...` and `locks/...` | Conditional ownership, expiry, generation, and fencing state. |
| `capacity/<consumer-id>.json` | Per-consumer resource broadcast. |
| `control/...` | Pause, drain, migration, and coordinator control state. |
| `config/quotas.json` | Provider-neutral reservation overlay. |
| `registry.json` | Versioned compute-target and coordinator registry, plus the `service_directory` and `placement_profiles` blocks. Each target may carry `managed_versions`, the declared version of every stado-managed binary on that host. Top-level keys a reader does not model round-trip verbatim, so a write never deletes them. |
| `system/storage-layout.json` | Versioned storage-layout marker. |

The queued-to-running claim is create-if-absent and therefore has one winner.
Other prefix transitions write the new record before deleting the old one.
Readers tolerate the resulting retry window and resolve terminal state first;
writers are idempotent and fenced by the expected generation.

## Scheduling rules

The Rust scheduler reads a bounded provider-neutral queue window, orders work by
priority and creation time, and admits only targets whose declared
capabilities, policy, deadline, resource envelope, and provider fence match.
Capacity broadcasts prevent dispatch when an existing eligible consumer can
accept the work. Cloud placement applies configured quota reservations, cost
policy, zone candidates, spot/on-demand policy, and retry backoff before an
owned provider mutation.

The local Rust agent publishes native capacity and scans for an eligible job.
It claims through the atomic storage primitive before writing runtime state or
starting a process. GPU admission uses free per-device VRAM plus any explicit
slot cap; CPU-only work uses the declared RAM, disk, deadline, and exclusivity
constraints. A paused queue prevents new claims while active slots continue to
heartbeat, yield, complete, fail, or cancel.

## Cloud-agent VM lifecycle

- **Spawn** — the Rust scheduler calls only an explicitly enabled provider
  adapter after identity, quota, network, release, and credential preflight.
- **Boot** — the adapter supplies a checksum-pinned Rust release and no
  application bearer or ambient cloud credential to the workload.
- **Run** — the Rust agent stages declared `stado://` inputs, resolves only
  allowlisted Skarbiec item/field references, claims eligible work, and writes
  canonical output.
- **Release** — the owning adapter releases its resource. Provider-specific
  metadata and cloud APIs remain encapsulated inside that adapter; local and
  consumer operator paths never invoke a provider CLI.

## Box sandbox lifecycle

Box is an explicitly selected, provider-pinned CPU sandbox path; it does not
run through the GPU startup-agent scheduler. The Box dispatcher performs
fixed-shape capability admission, then acquires a conditional object-store
lease before every provider mutation. Each invocation has a unique owner and
fencing token. Normal ticks relinquish ownership; a crashed tick remains fenced
until its owner lease expires.

The durable lifecycle is `allocating → provisioning → ready → starting →
running → collecting → releasing → released`, with a recoverable failure path.
The opaque Box identifier, launch intent, prompt identifier, terminal outcome,
and output state are persisted before the next external mutation. Commands use
a relative `.stado/<job>/` namespace, a create-once launch marker, detached
process group, bounded logs, optional verification command, and declared
bounded artifacts. Prompt jobs reconcile the official nested prompt-run status
and collect only final response events matching their prompt task identifier.
Resources are renewed only through active execution states and are archived or
deleted only after output persistence. Every terminal state is restartable, so
a coordinator crash cannot silently duplicate a launch or strand collection.

## Consumer capacity broadcasts

Every local or cloud agent writes
`stado://capacity/<consumer-id>` through the configured canonical backend on
each poll cycle:

```json
{
  "consumer_id": "local-ubuntu-server",
  "kind": "local",
  "free_vram_gb": 14,
  "total_vram_gb": 96,
  "free_slots": {"nvidia-tesla-t4": 6, "nvidia-l4": 4, "nvidia-tesla-a100": 2},
  "published_at": "2026-04-29T01:30:01.000000+00:00"
}
```

The cloud scheduler reads these to decide whether to *yield* a queued
job to a free local consumer instead of dispatching a paid VM. The
yield decision uses the cost-optimal knapsack: jobs with the highest
`$-saved-per-GB-of-local-VRAM` get marked for local pickup first.

## Declared host state and reconciliation

The registry is the fleet's declaration of what SHOULD be true, in two
places:

- `targets[].managed_versions` — the version each stado-managed binary
  (`stado`, `skarbiec`) is required to be at on that host. Optional: a
  target that omits it declares nothing, and is reported as `undeclared`
  rather than as agreeing.
- `service_directory.services[].endpoints[<target>]` — the endpoint that
  host answers on for a service, whether or not it is the active host.

`stado host inventory <target>` is the observation, and it reconciles the
two along axes that are deliberately independent of each other:

| Axis | Question | States |
|---|---|---|
| Marker vs listener | Is anything listening where `$HOME/.stado/forwards/<name>.url` points? | `matched`, `stale`, `unreadable`, `unknown` |
| Marker vs registry | Does that marker point where `service_directory` declares this host answers? | `matched`, `disagrees`, `undeclared` |
| Binary vs registry | Is the installed binary at the version `managed_versions` requires? | `matched`, `behind`, `ahead`, `mismatched`, `undeclared`, `unknown` |

Keeping the first two apart is the whole point. A marker can be `matched`
against the socket table and `disagrees` against the registry at the same
time — on `charless-mac-mini`, `skarbiec-weles` names `8895`, something is
listening on `8895`, and the directory declares `19095`. Every liveness
check passes, and every consumer that resolves through the directory lands
somewhere the marker never mentioned. Collapsing the two axes into one
"drift" verdict hides exactly that case.

Detection precedes automatic delivery, in that order and on purpose.
Software that installs builds onto hosts without being able to state the
declared version, read the actual version, and name the difference is not
automation; it is a faster way to break production. The declaration
(`managed_versions`) came first, the visibility (`host inventory`
reconciliation) second, and delivery is built on top of both rather than
beside them.
