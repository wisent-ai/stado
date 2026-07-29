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
| `queue/<id>.json` | Pending jobs. The ledger. |
| `running/<id>.json` | In-flight jobs with `instance_ref`, `started_at` set. |
| `completed/<id>.json` | Finished successfully. `completed_at` set. |
| `failed/<id>.json` | Finished with rc != 0. `failed_at`, `error` set. |
| `status/<id>/status` | `RUNNING <ts>` / `COMPLETED` / `FAILED exit=N`. |
| `status/<id>/heartbeat` | `RUNNING <ts>` refreshed by the running job. |
| `status/<id>/output/...` | Canonical stdout, profiles, and job artifacts published by the trusted agent. |
| `capacity/<consumer-id>.json` | Per-consumer broadcast: `free_vram_gb`, `total_vram_gb`, `free_slots`. |
| `scripts/<id>.sh` | Per-job rendered startup script (legacy 1-VM-per-job path). |
| `config/quotas.json` | Reservation overlay (subtracts from live API limits). |
| `registry.json` | Live compute-target registry; agents re-fetch every poll. |

State transitions are atomic from the caller's POV:
`write_job(new_prefix)` then `delete_blob(old_prefix)`.

## Scheduling rules

The scheduler (`stado/scheduler/scheduler.py`) sorts queued jobs
by `(-priority, created_at)` and applies, in order:

1. **Per-tick listing cap** — `_dynamic_per_tick_cap(queue_depth) * 8`
   blobs are pulled from GCS oldest-first. Beyond that wouldn't dispatch
   anyway, and pulling a 28k+ queue every tick blew the function timeout.
2. **Per-accelerator fairness** — `per_accel_share = ceil(per_tick_cap /
   distinct_accels)` so a heterogeneous queue (T4 + A100-40 + A100-80)
   makes concurrent progress instead of one accel hogging every tick.
3. **Cost-optimal local pack** — a `(wall_seconds/3600) * $/hr / vram`
   knapsack picks queued jobs to *yield* to a free local consumer
   instead of paying for a fresh VM.
4. **Spot with on-demand escape** — after `max_preempts_before_ondemand`
   preemptions (default 3), the next attempt for that job dispatches
   on-demand instead of Spot.
5. **Per-machine-type zone rotation** — `MACHINE_TYPE_ZONES` in
   `config.py` lets a SKU walk to alternate regions when its primary
   zones are exhausted (e.g. `a2-ultragpu-1g` → us-east5, europe-west4
   when us-central1 spot is dry).
6. **Dispatch backoff** — failed `create_instance` calls escalate via
   `DISPATCH_BACKOFF_MINUTES = [0, 1, 5, 15, 30, 60, 120, 240]`
   minutes per attempt count.

The local agent (`stado/providers/local_agent.py`) walks the
queue FIFO and claims any job whose `gpu_mem_gb <= free_vram_gb` AND
passes `_job_eligible` (gpu_type-compat or pinned-local). No slot count
— pure VRAM admission.

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

Every consumer (local agent + each cloud agent) writes
`gs://$WC_BUCKET/capacity/<consumer-id>.json` every poll cycle:

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
