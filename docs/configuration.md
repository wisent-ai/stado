# Configuration

## Deployment profile and bounded overrides

`STADO_CONFIG` selects the authoritative JSON profile. The shipped
`deploy/local/stado.config.json` is the active outage profile;
`deploy/azure/stado.config.json` is a fenced production template. Provider
order, explicit provider fences, storage, object/release/service verifiers, and
the workload-agent allowlist live in that profile rather than in cloud CLI state.

Only route-local or process-local values should be overridden:

| Var | Purpose |
|---|---|
| `STADO_CONFIG` | Readable deployment profile path. |
| `STADO_API_URL` | Stado HTTPS control origin; plain HTTP is accepted only on loopback. |
| `STADO_API_TOKEN` | Dedicated caller token for its mapped object namespace. |
| `STADO_MACHINE_API_TOKEN` | Machine submit/status/cancel token. |
| `STADO_SERVICE_API_TOKEN` | Caller-specific deployer token; accepted only for mapped service names/actions. |
| `STADO_RELEASE_API_URL` | Public HTTPS Stado origin serving `/api/release/object`. |
| `STADO_RELEASE_VERSION` | Required exact immutable Stado runtime version. |
| `STADO_RELEASE_PLATFORM` | Required exact Stado runtime platform for dispatched agents. |
| `WC_LOCAL_SLOTS` | Optional local-agent concurrency cap; `0` is uncapped. |
| `STADO_HOST_HEALTH_API_URL` | Authenticated Stado host-health origin. |
| `STADO_HOST_HEALTH_SKARBIEC_URL` | Skarbiec origin for the route-only host-health publisher. |
| `STADO_HOST_HEALTH_SKARBIEC_CONSUMER` | Exactly `stado-host-health-beacon`. |
| `STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE` | Owner-only grant scoped only to `stado-host-health-api`. |

Cloud-provider locators and credentials are not caller overrides. An enabled
provider adapter receives its exact profile and provider-plugin identity; a
workload-agent grant contains only the provider-neutral application items in
`agent.skarbiec.items`. It must never contain `stado-gcp`, `stado-azure`, or
`stado-aws`, and no bootstrap, health, recovery, or release path invokes
`gcloud`, `gsutil`, or `az`.

Product data enters through `stado://<namespace>/<key>` and the authenticated
Stado object boundary. Immutable artifact manifests may additionally reference
provider-native `az://`, `gs://`, and `s3://` locations, plus `hf://` and
HTTPS; access still resolves through authenticated provider adapters, and
embedded credentials or sensitive query parameters are rejected.

## Registry

`stado-rs/data/registry.json` is the canonical create-if-absent seed. It
declares the sole `local-control-plane` coordinator and only current,
host-backed local targets. Fenced Azure/GCP coordinators and cloud spot targets
must not remain marked active. Operators use `stado registry push` and
`stado registry pull`; both resolve the backend from `STADO_CONFIG`, preserve
generation fencing, and surface an unreachable store as failure rather than
silently switching providers.

Each target entry:

```jsonc
{
  "name": "my-workstation",
  "kind": "local",
  "ssh": "user@host-or-ip",       // used by `wc bootstrap` to install the agent
  "gpu_type": "nvidia-tesla-t4",  // SKU label the agent broadcasts
  "slots": 0,                     // 0 = no concurrency cap, pure VRAM admission
  "vram_gb": 96,                  // total GPU VRAM
  "env_overrides": { "WISENT_DTYPE": "auto" },
  "agent_args": ["--gpu-type", "nvidia-tesla-t4"]
}
```

A coordinator entry pins the scheduling-tick driver:

```jsonc
{
  "name": "local-control-plane",
  "runtime": "daemon",
  "host": "https://stado.wisent.com",
  "interval_seconds": 180,
  "state_uri": "stado://system/registry",
  "active": true
}
```

## Quotas

Live limits come from the GCP regions API
(`compute_v1.RegionsClient().get(project, region)`).
The provider-neutral `config/quotas.json` object in the configured Stado store
contains *reservations only*:
```json
{
  "gcp": {
    "nvidia-tesla-a100": {"reserved": 4}
  }
}
```

means "subtract 4 A100s from the live limit before dispatching" —
useful when you want to reserve capacity for non-wisent workloads.
The `total` field is ignored; setting it has no effect.

The metric-name → internal accel mapping
(`stado/scheduler/quota.py:_GCP_METRIC_TO_ACCEL`):

```python
_GCP_METRIC_TO_ACCEL = {
    "PREEMPTIBLE_NVIDIA_T4_GPUS":      "nvidia-tesla-t4",
    "PREEMPTIBLE_NVIDIA_L4_GPUS":      "nvidia-l4",
    "PREEMPTIBLE_NVIDIA_A100_GPUS":    "nvidia-tesla-a100",
    "PREEMPTIBLE_NVIDIA_A100_80GB_GPUS": "nvidia-a100-80gb",
}
```

## Optional GCP adapter prerequisites

Stado does not ship an infrastructure-provisioning shell path. An operator who
explicitly enables the GCP provider adapter must supply an already provisioned
project, shared store, identity, quota overlay, networking, and alert sink in
the deployment profile. Runtime compute and storage operations then remain
inside the Rust provider adapter; install, bootstrap, release, health, and
recovery paths never invoke a cloud CLI or consume ambient ADC.

The active deploy workflow publishes through Stado and installs the native
coordinator. It has no Cloud Function redeploy, provider CLI authentication,
workload-identity publisher, or ambient ADC path.

## Per-machine-type zone rotation

`MACHINE_TYPE_ZONES` in `stado/config.py` is consulted by
`providers/gcp.py:create_instance` before the default
`ZONE_ROTATION`. It exists because some accelerator-optimized SKUs
(`a2-ultragpu-1g`, the A100-80GB single-GPU machine) only exist in a
subset of zones, and Spot capacity in `us-central1-a` is regularly
exhausted:

```python
MACHINE_TYPE_ZONES = {
    "a2-ultragpu-1g": [
        f"{REGION}-c", f"{REGION}-a",
        "us-east5-a", "us-east5-b", "us-east4-c",
        "europe-west4-a",
    ],
}
```

The provider iterates these in order and creates the instance in the
first zone that returns a non-`None` ref. 503 ZONE_RESOURCE_POOL_EXHAUSTED
or 400 "machine type does not exist" cause it to walk to the next zone.

## Pinned cloud-agent dependencies

`stado/templates/startup_gpu_agent.sh` pins the following
deps. Each pin has a known reason — don't relax them without reading
the comments in the template:

| Pin | Reason |
|---|---|
| `transformers>=4.55,<5.0` | transformers 5.x has a 0-indexed shard-name miscompute that fails on Llama-2-7b/Qwen3-8B/gpt-oss-20b. |
| `tokenizers>=0.20,<0.22` | matches `transformers<5.0`. |
| `datasets>=3.0,<4.0` | datasets 4.0 dropped support for dataset loading scripts (`flores.py` etc. raise `RuntimeError: Dataset scripts are no longer supported`). |
| `huggingface-hub>=0.34.0,<1.0` | hub 1.x violates `transformers<5.0`'s `huggingface-hub<1.0` constraint and the agent crashes at import time. |
| `numpy>=1.24,<2.3` | numba 0.61.x requires numpy < 2.3. |
| `NUMBA_NUM_THREADS=1` (env var) | wisent sets this in 8 modules but the import-order race lets numba init at the system cpu_count first; setting it in the agent's own env avoids `RuntimeError: Cannot set NUMBA_NUM_THREADS once threads have been launched`. |
| `HF_HUB_DOWNLOAD_TIMEOUT=120` (env var) | wisent fleet hits HF's 1000-req/5-min free-tier ceiling regularly; longer timeouts let the SDK back off and retry rather than fail the whole job. |
