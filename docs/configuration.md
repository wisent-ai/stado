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
| `STADO_ALERT_CHANNELS` | Explicit comma-separated optional adapters: `slack`, `telegram`, `sendgrid`, `resend`, `most`, `gcp-pubsub`. |
| `WC_LOCAL_SLOTS` | Optional local-agent concurrency cap; `0` is uncapped. |
| `STADO_HOST_HEALTH_API_URL` | Authenticated Stado host-health origin. |
| `STADO_HOST_HEALTH_SKARBIEC_URL` | Skarbiec origin for the route-only host-health publisher. |
| `STADO_HOST_HEALTH_SKARBIEC_CONSUMER` | Exactly `stado-host-health-beacon`. |
| `STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE` | Owner-only grant scoped only to `stado-host-health-api`. |
| `STADO_CREDENTIALS_STORE` | Requested credential backend (`skarbiec`, `skarbiec://<https-origin>`, or `file://<absolute-path>`). A value different from `credentials.store` is a pending migration. |
| `STADO_CREDENTIALS_ADMIN_CONSUMER` | Skarbiec bootstrap consumer used only for store administration and migration. |
| `STADO_CREDENTIALS_ADMIN_TOKEN_FILE` | Owner-only bootstrap grant for the credential-store administrator. |

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

Channel material is split by what it is. Secrets stay in Skarbiec: the
`stado-alerts` item holds `slack_webhook`, `telegram_bot_token`,
`telegram_chat_id`, `sendgrid_api_key` and `most_phone`, while the `resend`
channel reads its key from the item named by `alerts.resend_item` (field
`alerts.resend_field`, default `RESEND_API_KEY`/`value`) so a deployment can
point at whichever item actually holds a live key. Destinations are not
secrets and live in the config document: `alerts.email_to` and
`alerts.email_from`, still overridable by `WC_EMAIL_TO` and `WC_EMAIL_FROM`.
The sender must be a domain the provider has verified, or the send is
rejected at delivery time.

`stado alerts channels` prints which channels resolved and where each one
lands; `stado alerts send "<message>"` pages them on purpose, which is the
only evidence that alerting works before an incident needs it.

## Credential store

One selector owns every application credential, including cloud/provider
credentials, service tokens, and SSH host keys:

```json
{
  "credentials": {
    "store": "skarbiec",
    "admin": {
      "consumer": "local-operator",
      "token_file": "~/.stado/local-operator-skarbiec-token"
    }
  }
}
```

`credentials.store` is the committed source of truth.
`STADO_CREDENTIALS_STORE` is its process-level override. Supported locators are
`skarbiec`, `skarbiec://<https-origin>`, and `file://<absolute-path>`. The file
backend is an owner-only local/offline manager; Skarbiec adds encryption,
scoped grants, audit, recovery recipients, and remote HTTPS access.

Changing the environment selector does not make Stado read an empty backend.
It creates a fail-closed pending migration:

```sh
export STADO_CREDENTIALS_STORE=file:///secure/stado-credentials.json
stado secrets migrate
```

Without an environment override, `stado secrets migrate --to <locator>` performs
the same change. Migration snapshots every active item and its type, requires an
empty destination, copies and reads every value back, commits
`credentials.store`, and only then removes the source items. A failed copy,
verification, config write, or source cleanup rolls back to the previous store.
Normal reads and writes remain blocked while the environment and committed
selectors differ.

All `stado secrets` CRUD, provider reads, scoped verifier reads, and
`stado_fleet key` operations use this selector. There is no OpenSSH-file
fallback for host channels. Only a backend's own bootstrap credential remains
outside the selected store: putting the grant needed to unlock a manager inside
that same manager would be circular. For Skarbiec, this is the owner-only admin
token file named above.


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

A registry document may also carry the fleet's central enrollment and
communication catalog. `enrollment` declares which registration paths are
allowed — `allow_join` for machine-initiated `stado_fleet join`/`approve`,
`allow_enroll` for control-plane `stado_fleet enroll`, and
`require_verified_hostname` for the verified-identity contract.
`channels` declares how machines reach the control plane. Both sections
are additive: a document without them is unrestricted, which
`stado_fleet catalog` reports explicitly. The `stado_fleet` commands
enforce the catalog in their preflights, before any write.

Enrollment has two registry states. `provisioning_targets` is an internal,
non-routable staging section visible only to agent bootstrap; its entries
cannot belong to a fleet. After the agent publishes fresh capacity, Stado
atomically moves the entry to `targets` with an `agent_enrollment` receipt
containing the consumer identity, hostname, target kind, Stado version, and
publication time. Set `enrollment.require_agent_attestation` only after
legacy local targets have been reconciled; once true, registry validation
refuses every local target without a valid receipt.
`stado_fleet enforce-attestation` performs that cutover atomically and fails
until every local target has a receipt.


```jsonc
{
  "enrollment": {
    "allow_join": true,
    "allow_enroll": true,
    "require_verified_hostname": true,
    "require_agent_attestation": true
  },
  "channels": {
    "control_plane": ["loopback"],
    "notes": "any address that resolves: LAN, mDNS, tailnet"
  }
}
```

## Local inference

The optional top-level `inference` section is the single desired-state and
routing catalog for Stado-managed vLLM. `gateway_target` is the registered host
running Brama; deployments run on registered local GPU targets and expose their
OpenAI-compatible endpoint only on a Tailscale IPv4 address.

```jsonc
{
  "inference": {
    "gateway_target": "my-brama-host",
    "deployments": [],
    "routes": {}
  }
}
```

The lifecycle is deliberately two-step and generation-fenced:

Create the shared bearer without exposing it in argv, output, or a file:

```sh
stado inference init-credential
```


```sh
stado inference plan chat-primary \
  --host ubuntu-server-rtx-pro-6000 \
  --image 'vllm/vllm-openai@sha256:770fe65b2c73ee74a5c42165cf3433de4048cc2cd9c57a937ca4e35aba5aa87b' \
  --cache-dir /mnt/wd16tb/stado/inference/chat-primary \
  --model 'TheDrummer/Cydonia-24B-v4.3' \
  --revision 'db0426d39d4bd4a6d34fdc71db97569da68f55e1' \
  --max-model-len 16384
stado inference apply <plan-id>
stado inference doctor chat-primary
stado inference verify chat-primary
stado inference route set wisent-backend/chat/primary \
  --to chat-primary --expected absent \
  --gateway ubuntu-server-rtx-pro-6000
```
This revision-pinned BF16 model is the same production model used by the former
GCP A100 deployment. The immutable Hugging Face revision and amd64 vLLM image
digest prevent silent model or runtime replacement.

If `plan` or `apply` reports an unmanaged GPU workload, inspect it through the
same target-scoped host channel instead of opening an ad hoc SSH session:

```sh
stado inference blockers --host ubuntu-server-rtx-pro-6000
stado inference release --host ubuntu-server-rtx-pro-6000 \
  --identity <PID:START_TICKS>
```

`blockers` reports the executable, owner, VRAM use, cgroup, and an identity made
from both PID and `/proc` start ticks. `release` refuses a stale identity, sends
`TERM`, and waits for exit. Add `--force` only to escalate that same verified
process to `KILL`; it never accepts a bare PID.

A cancelled or failed pre-commit plan can leave a runtime or root-owned model
cache without a registry deployment. Clean that exact saved plan through the
managed channel:

```sh
stado inference plan-logs <plan-id>
stado inference abort <plan-id> --purge-cache
```

`plan-logs` reads the not-yet-committed container logs through the same managed
host channel.

`abort` never changes the registry. It stops only the runtime described by the
immutable local plan, removes its cache through the pinned container runtime,
and consumes the plan after successful cleanup. Set `--cache-dir` during
`plan` when the target's home filesystem is not the intended model volume.


`plan` inventories the host, requires Docker, NVIDIA tooling, and a live
Tailscale address, then saves an immutable plan bound to the current registry
digest. `apply` rechecks that precondition, installs the digest-pinned vLLM
container under Docker's `unless-stopped` supervisor, waits for an authenticated
readiness probe, and only then commits the deployment. A failed runtime,
readiness check, or registry compare-and-swap restores the prior runtime.

The deployment and Brama use one centrally stored credential item:
`provider:local-openai`, containing a non-empty `token` field.
`stado inference init-credential` generates and stores it without printing the
token, and refuses to overwrite an existing item. Deliberate rotation can use
`stado credentials put provider:local-openai` with JSON on standard input, but
must be coordinated with runtime replacement; never place the token in argv or
registry data. Route changes require `--expected`, probe the destination first,
stage an owner-only route snapshot on the gateway, compare-and-swap the
registry, and then atomically commit the snapshot. Brama reloads that file per
request, so cutover needs no backend restart. Ordered `--fallback` destinations
are attempted when the primary provider fails; `qwen/default` therefore
preserves service while local vLLM is unavailable. `rollback` reinstalls the
recorded prior deployment; `retire` refuses while any primary or fallback route
still selects the deployment and retains model cache unless `--purge-cache` is
explicit.


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
