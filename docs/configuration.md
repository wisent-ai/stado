# Configuration

## First-run config and deployment profiles

`stado config init` creates only a schema-versioned local queue profile:
local compute, local primary and backup stores, one deployment identity, and a
loopback dashboard. It contains no Wisent service routes, production clients,
cloud locators, or credentials. Existing legacy files migrate explicitly with
`stado config migrate`; the exact prior file is preserved beside the migrated
document.

`STADO_CONFIG` selects an authoritative deployment profile. The shipped
`deploy/local/stado.config.json` is an explicit Wisent outage profile, not a
first-run template; `deploy/azure/stado.config.json` is a fenced production
template. Provider order, explicit provider fences, storage,
object/release/service verifiers, and the workload-agent allowlist live in
deployment profiles rather than in cloud CLI state.

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
| `STADO_ALERT_CHANNELS` | Explicit comma-separated optional adapters: `slack`, `telegram`, `sendgrid`, `gcp-pubsub`. |
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

Optional alerts are disabled when `alerts.channels` is absent or empty. Enabling
a channel authorizes only its own credential lookup and network route. The
Pub/Sub topic and SendGrid recipient are inert unless their adapters are also
enabled. An alert failure is isolated from scheduling, execution, health, and
the other channels.

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

The provider-neutral `config/quotas.json` object contains reservation overlays,
not invented capacity. A reservation subtracts operator-owned capacity from a
live provider limit before dispatch. Missing or unreachable live quota data is
reported as unavailable; Stado does not reinterpret it as zero usage or
unlimited capacity.

GCP quota reads are part of the preview compute adapter and use the Rust
provider boundary. Azure supports configured reservations with incomplete live
VM-family coverage. Live AWS quota management is planned and unavailable.

## Optional GCP adapter prerequisites

The GCP compute adapter is preview. An operator must explicitly enable it and
supply an already provisioned project, canonical store, scoped managed identity
or Skarbiec service account, quota overlay, network, image, ownership labels,
cost policy, and immutable Stado release. Runtime compute and storage operations
stay inside the Rust provider adapters; install, bootstrap, release, health,
and recovery paths do not invoke a cloud CLI.

Zone candidates and machine compatibility belong to the selected deployment
profile. Resource exhaustion may advance to the next allowed candidate; an
authorization, ownership, invalid configuration, or ambiguous provider error
must not. GCP is not stable until a release-scoped live test creates an owned
VM, boots the pinned agent, runs and collects a workload, exercises
cancellation and recovery, and reaps all paid resources.

The active deployment workflow publishes immutable native releases and
installs the Rust coordinator. It has no Cloud Function scheduler, mutable
package upgrade, provider CLI authentication, or ambient credential fallback.

## Workload runtime ownership

Stado does not pin Python, CUDA framework, Hugging Face, NumPy, or application
package versions as part of the control-plane contract. A workload declares and
owns its runtime, image, command, source revision, artifacts, and verification
hook. Provider bootstrap templates may install prerequisites required by a
specific workload profile, but those pins are deployment data and do not become
dependencies of the Rust agent or local onboarding path.
