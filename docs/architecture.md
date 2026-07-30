# Architecture

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
| `registry.json` | Versioned compute-target and coordinator registry. |
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
