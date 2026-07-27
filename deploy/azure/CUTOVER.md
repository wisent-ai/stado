# Azure control-plane cutover

`deploy/azure/stado.config.json` is the fenced production destination. Azure
activation remains blocked while the tenant-root `UnusualActivity` deny,
storage account, managed identity and valid S3 credentials are unresolved.
Neither the coordinator nor the workflow silently selects GCP.

## Outage-safe Mac control plane

The active self-hosted Mac path uses `deploy/local/stado.config.json`. It keeps
the intended provider order visible but explicitly disables Azure, AWS and GCP.
The only active provider is `local`; the primary store is
`~/.stado/local-storage` and synchronous/read-fallback replication targets the
distinct `~/.stado/local-backup` path. This is same-disk temporary protection,
not the required cross-provider disaster recovery.

Unless repository variable `STADO_ENABLE_AZURE_DEPLOY` is exactly `true`, the
deployment workflow skips every Azure upload/login and installs this local
profile. It creates both owner-local directories and seeds the canonical
registry only when absent. Rust `stado bootstrap --local` installs the combined
local coordinator, agent, dashboard, object API and machine API behind Caddy.
The object API still fails closed unless the `stado-control-plane` grant can
read field `token` from item `stado-object-api`; the machine API independently
requires field `token` from item `stado-machine-api`.

Do not enable `STADO_ENABLE_AZURE_DEPLOY` or point the Mac at the production
template until every resource below exists and `stado doctor` reports it ready.

Stado never provisions external cloud resources. Provision the items below
before running the deployment workflow:

- An Azure storage account with a private `stado` Blob container.
- Per-region virtual networks, subnets, security groups and approved GPU
  quota matching `deploy/azure/stado.config.json`.
- A user-assigned Azure managed identity attached to every agent VM. Grant it
  Blob Data Contributor on the storage account and the least VM-delete rights
  needed for idle self-termination.
- An S3 disaster-recovery bucket in the configured region.
- A GitHub federated Azure publisher identity with write access to the release
  container, plus the repository variables and secrets named in
  `.github/workflows/deploy.yml`.
- A self-hosted GitHub runner labelled `stado-control-plane`.
- Reachable Skarbiec service and least-privilege service grants.
- TLS DNS and a reverse proxy based on `deploy/azure/Caddyfile.example`.

## Credential boundaries

Skarbiec is the only source of application credentials. Do not copy values
into this repository, GitHub variables, cloud secret managers or
`stado.config.json`.

The coordinator/dashboard uses consumer `stado-control-plane` and owner-only
grant file `~/.stado/control-plane-skarbiec-token`. Individual code paths read
only their own fields: item `stado-azure` for off-Azure ARM and Blob access,
item `stado-aws` for coordinator-side S3 replication, field `token` from item
`stado-object-api` for object API calls, and field `token` from item
`stado-machine-api` for machine submit/status/cancel. Inspect a field only with
`stado secrets get ITEM --field FIELD`; never parse a whole-item JSON response.
Workload-specific items are read only when their dispatch path needs them.

The Azure agent grant is a different consumer, `stado-azure-agent`. Its exact
`item:read` allowlist is `agent.skarbiec.items`: `stado-aws` plus every item
referenced by an allowed workload's `secret_env`. Its owner-only grant lives
at `~/.stado/azure-agent-skarbiec-token`. Before dispatch, Stado requires the
grant's visible item set to equal that configuration exactly, then delivers
the opaque grant through a single-read FIFO into the Rust agent's in-memory
cache. The FIFO disappears; workload processes inherit neither the grant nor
unrequested secret values.

`STADO_API_TOKEN` is the object API token for trusted publisher services only.
Read it with `stado secrets get stado-object-api --field token` under that
service's scoped grant. `STADO_MACHINE_API_TOKEN` is the separate machine API
token; read only field `token` from item `stado-machine-api`. Neither token can
authorize the other's routes, and machine authorization never falls through to
the dashboard Supabase grant. A model-router service instead reads field
`token` from item `stado-model-router`; all three tokens are distinct,
server-side credentials.

Submitted machine-job processes receive none of `STADO_CONFIG`,
`STADO_API_TOKEN`, `STADO_MACHINE_API_TOKEN`, Skarbiec routing, Azure/AWS
credentials or provider storage locators. Submitters declare provider-neutral
`input_objects`; jobs write under `output/`; the trusted Rust agent alone stages
inputs and publishes canonical output.

## Trusted machine API

Trusted services use `STADO_API_URL=https://stado.wisent.com` and send
`Authorization: Bearer $STADO_MACHINE_API_TOKEN`. Remote DNS hosts are accepted
only through the configured HTTPS reverse proxy, which must preserve `Host` and
supply `X-Forwarded-Proto: https`. Missing or unreadable credentials, an
untrusted host/protocol, and failed authorization are closed failures.

The authenticated interface has exactly three routes:

- `POST /api/machine/submit` with `Content-Type: application/json`, an exact
  `Content-Length`, and at most 64 KiB of canonical machine-request JSON.
  `source_archive_path` is forbidden because a remote caller must never name a
  coordinator-local file. Payload files are uploaded through the object API and
  declared in `input_objects` as `stado://` locators with relative job paths
  (and an optional SHA-256).
- `GET /api/machine/status?job_id=ID` with exactly one path-safe `job_id` and no
  request body.
- `POST /api/machine/cancel?job_id=ID` with exactly one path-safe `job_id` and
  no request body.

Successful calls return `{"ok":true,"result":...}` around the canonical
`MachineFacade` result. Facade failures retain their stable
`{"ok":false,"error":{"code":...,"message":...,"retryable":...}}` shape.
Malformed JSON, framing, or query data is rejected before any machine action.
The request and response contain no cloud credentials or provider-native object
locators; object bytes continue to move through the `stado://` object API.

## Configure one deployment

Copy `deploy/azure/stado.config.json` to `~/.stado/config.json`, replace every
placeholder, and leave provider/storage selection in that file rather than
duplicating it in process environment. `deploy/azure/env.example` contains
only the config-file pointer and credential-boundary guidance.

The load-bearing values are:

- `providers` records preferred order `azure`, `aws`, `gcp`; `providers_disabled`
  explicitly fences AWS until its network/AMI is known and GCP while billing is
  disabled. The scheduler sees only unfenced entries and never falls back.
- `storage.backend` is `azure`, with the provisioned account and `stado`
  container.
- `storage.backup.backend` is `s3`, with its bucket and region.
- `azure.vm_identity_id` names the user-assigned managed identity.
- `release.base_url` explicitly names the Azure Blob release tree. There is
  no built-in or provider-derived release origin.
- `deployment.id` is stable for dashboard RLS and trusted-proxy binding.
- Both coordinator and agent Skarbiec routes name their separate consumers.

Queue and product-object mutations always target Azure. S3 replication is
synchronous for product objects and refreshed by every coordinator tick for
canonical state. A failed Azure read may use S3 read fallback, but Stado
enters a safe read-only posture: it never promotes S3 to writer
automatically.

## Publish the Rust release

The active `.github/workflows/deploy.yml` builds Rust binaries, authenticates
the Azure publisher through GitHub OIDC, and publishes:

```text
releases/stado/latest.json
releases/stado/<version>/linux-amd64/<binary>
releases/stado/<version>/linux-amd64/SHA256SUMS
config/quotas.json
```

The second workflow job runs on the dedicated control-plane runner, builds
the native Rust binaries and calls `deploy/deploy_stado_rust.sh`. That script
does not provision infrastructure or invoke Python deployment code. It gates
installation on `stado config validate` and `stado doctor`, then delegates
all service rendering to `stado bootstrap --local`.

For a first manual workstation install, set `WC_RELEASE_BASE_URL` to a
pre-authenticated Azure Blob URL, such as a narrowly scoped container SAS,
and run `deploy/stado-up.sh <target>`. The shell installer exists only to
obtain Rust Stado; Rust owns the persistent service after preflight.

## Public object route

Set `STADO_PUBLIC_BASE_URL=https://stado.wisent.com` in browser/read-only
client deployments. Public immutable objects use
`stado://public/<product>/...` and resolve through:

```text
${STADO_PUBLIC_BASE_URL}/api/public/object?uri=<urlencoded-stado-uri>
```

The reverse proxy must forward that exact path unchanged. The Rust handler
allows unauthenticated GET only in the `public` namespace; it exposes no
list, stat, mutation or private read. Never ship `STADO_API_TOKEN` to a
browser. Trusted publisher services use `STADO_API_URL` plus their
server-side `STADO_API_TOKEN`.

## Preflight and cutover

Run the Rust preflight before service installation:

```sh
stado config validate
stado doctor --fix-hints
```

Preflight fails with explicit remedies when the Azure account/container,
managed identity, S3 bucket/region or credentials, restricted agent grant,
object API token or release channel is unresolved. It checks actual Blob and
S3 access and fetches the same release pointer agents use.

Drain any old writer before seeding Azure. If GCP billing is temporarily
restored and retained data must be copied, the explicitly GCP-named
`stado storage copy --from gcs ... --to azure ...` operator command is the
only supported bridge. It is not called by either active workflow. After the
final copy, do not run GCP and Azure coordinators together.

Start the Rust coordinator through the deploy workflow or `stado-up.sh`.
Registry bootstrap then reads the canonical registry through configured
Stado storage and runs `stado bootstrap` for named local targets. It has no
GCS URL or release fallback.

## Resources that remain operator-owned

Deployment is blocked until the operator confirms:

- Azure account, private container and data-plane role assignments.
- Agent managed identity, networking and GPU quota in every configured
  region.
- S3 bucket, migrated `stado-aws` fields and separate control-plane/agent
  grants at the configured owner-only paths.
- Azure release publisher federation and an initially populated release
  channel.
- Self-hosted control-plane runner with Docker, Rust, Skarbiec reachability and
  a running Caddy service. Azure CLI is required only after explicitly enabling
  Azure publishing. The workflow otherwise derives the loopback dashboard
  upstream from resolved Stado config.
- DNS/TLS for `stado.wisent.com`; read-only clients set
  `STADO_PUBLIC_BASE_URL=https://stado.wisent.com`.

These are deliberately never auto-created by the coordinator; a missing item
must fail deployment rather than selecting GCP or creating a second writer.
