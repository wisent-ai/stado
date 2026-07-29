# Azure control-plane cutover

`deploy/azure/stado.config.json` is the fenced production destination. Azure
activation remains blocked while the tenant-root `UnusualActivity` deny,
storage account, managed identities, workload-only agent grant, and
cross-provider replica identity are unresolved. Neither the coordinator nor
the workflow silently selects GCP or invokes a cloud CLI.

## Outage-safe Mac control plane

The active self-hosted Mac path uses `deploy/local/stado.config.json`. It keeps
the intended provider order visible but explicitly disables Azure, AWS and GCP.
The only active provider is `local`; the primary store is
`~/.stado/local-storage` and synchronous/read-fallback replication targets the
distinct `~/.stado/local-backup` path. This is same-disk temporary protection,
not the required cross-provider disaster recovery.

Unless repository variable `STADO_ENABLE_AZURE_DEPLOY` is exactly `true`, the
deployment workflow skips every Azure operation and keeps the already-installed
local profile selected by `STADO_CONFIG`. The canonical registry is created
only when absent. Rust `stado bootstrap --local` installs the combined
local coordinator, agent, dashboard, object API, machine API and managed-service
control API behind Caddy. The local profile fixes `deployment.id` to
`local-control-plane`: loopback hosts remain accepted, but dashboard view and
operate requests still require deployment-bound Supabase authorization and fail
closed when that authorization is unavailable. Product object calls resolve
their canonical namespace and allowed key prefix, then require only that
namespace's `<namespace>-object-api/token`. Machine submit/status/cancel requires
`stado-machine-api/token`. Managed-service status/restart resolves the requested
service and action against `service_api.deployers`, then accepts only that
deployer's exact token.

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
- An S3 disaster-recovery bucket in the configured region, accessed only by
  the backup provider adapter's identity.
- The `stado-release-publisher` repository/service grant and immutable Stado
  release API route; no GitHub cloud-provider credential or direct bucket
  publisher is accepted.
- A self-hosted GitHub runner labelled `stado-control-plane`.
- Reachable Skarbiec service and least-privilege service grants.
- TLS DNS and a reverse proxy based on `deploy/azure/Caddyfile.example`.

## Credential boundaries

Skarbiec is the only source of application credentials. Do not copy values
into this repository, GitHub variables, cloud secret managers or
`stado.config.json`.

The coordinator/dashboard uses consumer `stado-control-plane` and owner-only
grant file `~/.stado/control-plane-skarbiec-token`. Provider credentials are
owned by exact provider-plugin consumers, never by this control-plane grant or
by a workload-agent grant. Product bearer verification
uses the separate consumer `stado-object-api-verifier` and owner-only grant file
`~/.stado/stado-object-api-verifier-skarbiec-token`. That grant must expose
exactly the `<namespace>-object-api` items declared in
`object_api.namespaces`; every item contributes only field `token`. Startup and
doctor reject missing or unexpected items, missing/empty fields, duplicate token
values, a reused coordinator grant, and an invalid namespace/item/prefix map.
Authenticated release creation uses a third consumer,
`stado-release-api-verifier`, and owner-only grant file
`~/.stado/stado-release-api-verifier-skarbiec-token`. Its visible set must equal
`release_api.publishers`: one `<product>-release-publisher/token` item per active
publisher. Those values must also be distinct from every product object bearer.
The authenticated `/api/object` path permits release GET/stat and prefix-scoped
list plus create-only PUT with `if_absent`; it rejects overwrite and delete.
Managed-service verification uses the fourth consumer,
`stado-service-api-verifier`, and owner-only grant file
`~/.stado/stado-service-api-verifier-skarbiec-token`. Its visible set must equal
the deployer items in `service_api.deployers`. Each deployer token is distinct
from all object, release, and other service tokens; its owning caller consumer
may read only that item. Unmapped service names/actions fail closed before any
credential is read.
Secret fields move through non-rendering owner-only projections; never print or
parse a whole item. Workload-specific items are read only when their dispatch
path needs them.

The shipped Azure profile carries a newly named workload-only consumer,
`stado-azure-workload-agent`, with exact provider-neutral
`agent.skarbiec.items` and `agent.skarbiec.secret_fields` mappings. The former
broad `stado-azure-agent` identity remains revoked and must not be recreated.
Storage and cloud-provider items are forbidden from the workload grant:
provider adapter identities own Azure and replica access. Stado validates the
consumer separation, exact visible item set, and item/field projection before
dispatch and fails closed on any drift.

`STADO_API_TOKEN` in each trusted server-side product deployment is projected
from that product's exact mapped item; the standard item is
`<namespace>-object-api/token`, while Wisent Backend uses the dedicated
`wisent-backend-object-client/token`. A product bearer is accepted only for the
matching namespace, one of its finite configured top-level directory prefixes
or exact top-level object keys, and its least-privilege action allowlist. No
active product has namespace-root object access. `STADO_MACHINE_API_TOKEN` is
projected only from `stado-machine-api/token` and authorizes machine
submit/status/cancel. Each caller's `STADO_SERVICE_API_TOKEN` instead comes from
its mapped `<deployer-item>/token` and authorizes only the configured service
names and actions. The verifier grant is
`stado-service-api-verifier`; no global service bearer, release publisher, or
dashboard Supabase authorization fallback exists.

Submitted machine-job processes receive none of `STADO_CONFIG`,
`STADO_API_TOKEN`, `STADO_MACHINE_API_TOKEN`, `STADO_SERVICE_API_TOKEN`,
Skarbiec routing, Azure/AWS credentials or provider storage locators. Submitters
declare provider-neutral
`input_objects`; jobs write under `output/`; the trusted Rust agent alone stages
inputs and publishes canonical output.

## Trusted machine and managed-service APIs

Trusted services use `STADO_API_URL=https://stado.wisent.com`. Machine routes
send `Authorization: Bearer $STADO_MACHINE_API_TOKEN`; managed-service callers
send their mapped deployer token as `STADO_SERVICE_API_TOKEN`. Stado resolves
the service name and action before reading the matching verifier item. Remote
DNS hosts are accepted only through the configured HTTPS reverse proxy, which
must preserve `Host` and supply `X-Forwarded-Proto: https`. Missing or unreadable
credentials, an unmapped service/action, an untrusted host/protocol, and failed
authorization are closed failures.

The authenticated interface has exactly five routes:

- `POST /api/machine/submit` with `Content-Type: application/json`, an exact
  `Content-Length`, and at most 64 KiB of canonical machine-request JSON.
  `source_archive_path` is forbidden because a remote caller must never name a
  coordinator-local file. Payload files are uploaded through the object API and
  declared in `input_objects` as `stado://` locators with relative job paths
  (and an optional SHA-256).
  A request with `repo` must also carry `repo_ref` as the full lowercase
  commit hash. The agent fetches that exact object, checks out detached, and
  verifies `HEAD`; a branch, tag, short hash, or missing ref is refused.
- `GET /api/machine/status?job_id=ID` with exactly one path-safe `job_id` and no
  request body.
- `POST /api/machine/cancel?job_id=ID` with exactly one path-safe `job_id` and
  no request body.
- `GET /api/service/status?name=NAME` with exactly one lowercase managed-service
  `name` and no request body. The response preserves the existing
  `deploy::service` beacon-status JSON array.
- `POST /api/service/restart?name=NAME` with exactly one lowercase
  managed-service `name` and no request body. It preserves the existing
  `deploy::service` restart semantics across every host declaring that name.

The machine bearer is rejected on both service routes, and the service bearer
is rejected on all three machine routes. Possession of one control credential
therefore grants no operation in the other family.

Successful calls return `{"ok":true,"result":...}` around the canonical
`MachineFacade` result or the existing service status/restart result array.
Failures retain the stable
`{"ok":false,"error":{"code":...,"message":...,"retryable":...}}` shape.
Malformed JSON, framing, or query data is rejected before any machine or
service action.
The request and response contain no cloud credentials or provider-native object
locators; object bytes continue to move through the `stado://` object API.

## Configure one deployment

Treat `deploy/azure/stado.config.json` as a fail-closed future cutover profile.
Replace every placeholder and provision every identity before copying it to
`~/.stado/config.json`. Provider/storage selection belongs only in that file;
`deploy/azure/env.example` contains credential-boundary guidance, not an
alternate configuration path.

The load-bearing values are:

- `providers` is the enabled set and stays empty in the shipped template;
  `providers_disabled` fences Azure, AWS, and GCP. Only after Azure's complete
  preflight passes, remove Azure from the disabled set and add it to
  `providers`; the scheduler never falls back.
- `storage.backend` is `azure`, with the provisioned account and `stado`
  container.
- `storage.backup.backend` is `s3`, with its bucket and region.
- `azure.vm_identity_id` names the user-assigned managed identity.
- `release.api_url`, `release.version`, and `release.platform` identify the
  exact immutable Stado runtime consumed through `/api/release/object`. There
  is no built-in, provider-derived, or mutable release origin.
- `deployment.id` is stable for dashboard RLS and trusted-proxy binding. The
  local profile also sets one explicitly; an absent id never opens dashboard
  view or operate access.
- The active local coordinator and standalone local agent name separate
  `stado-control-plane` and `stado-local-agent` consumers. Remote Darwin
  registry targets receive only the dedicated local-agent grant; bootstrap
  refuses a control-plane consumer or token path and preserves the exact
  item/field allowlists in the launchd environment.

After an approved cutover, queue and product-object mutations target Azure.
S3 replication is synchronous for product objects and refreshed by every
coordinator tick for canonical state. A failed Azure read may use S3 read
fallback, but Stado enters a safe read-only posture and never promotes S3 to
writer automatically.

## Publish the Rust release

The active `.github/workflows/deploy.yml` builds Linux and native Rust
binaries, resolves only `stado-release-publisher/token` through its dedicated
Skarbiec grant, and creates immutable objects through the Stado release API:

```text
stado://releases/stado/<version>/<platform>/<binary>
stado://releases/stado/<version>/<platform>/SHA256SUMS
```

Every PUT uses create-if-absent. A retry accepts an existing object only after
byte-for-byte comparison; different content at the same version URI is a hard
collision. No provider CLI, ambient cloud identity, mutable image tag, or
release pointer participates.

The control-plane job then installs the already-built local release through
`deploy/deploy_stado_rust.sh`. It does not provision infrastructure or edit the
SecretState-owned profile. First installs set `STADO_RELEASE_API_URL`, an exact
`STADO_RELEASE_VERSION`, and an exact `STADO_RELEASE_PLATFORM`, then run
`deploy/stado-up.sh <target>`.

## Software release route

The generic public object namespace does not exist. Public updater artifacts
live only under `stado://releases/<product>/...` and resolve through:

```text
https://stado.wisent.com/api/release/object?uri=<urlencoded-stado-uri>
```

The reverse proxy forwards that path unchanged. The Rust handler allows
unauthenticated GET only in the `releases` namespace; it exposes no list, stat,
mutation, or product-object read. Authenticated publishers receive only
`<product>-release-publisher/token` and may create immutable objects under exact
`stado://releases/<product>/` prefixes through `/api/object` with `if_absent`.
Overwrite, delete, cross-prefix list/read, product bearers, and the retired
global object bearer are rejected. All user/product objects remain on their
separately authenticated `/api/object` namespace or a signed proxy.

## Preflight and cutover

Run the Rust preflight before service installation:

```sh
stado config validate
stado doctor --fix-hints
```

Preflight fails with explicit remedies when the Azure account/container,
managed identity, S3 bucket/region or provider identity, restricted agent
grant, object namespace map, release publisher map, service deployer map, any
dedicated verifier grant, mapped token field, or public release channel is
unresolved. It rejects overbroad verifier item sets and duplicate bearer values
across product, release, and service families without rendering any value.
Cutover requires a non-empty `stado-machine-api/token`, every mapped
product/release/service token, and the exact consumer scopes documented above.

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
- The cross-provider replica is pre-provisioned and reachable only through
  Stado's storage adapter; no provider credential is present in an agent grant.
- A future Azure workload-agent grant must be newly scoped; the revoked legacy
  identity must not be recreated. The active local control-plane and local-agent
  grants remain distinct.
- Dedicated single-purpose workload items must match every configured
  `agent.skarbiec.secret_fields` entry. Never place deployment-wide or provider
  credentials in that field allowlist.
- Azure release publisher federation and an initially populated release
  channel.
- Self-hosted control-plane runner with Docker, Rust, Skarbiec reachability and
  a running Caddy service. Azure CLI is required only after explicitly enabling
  Azure publishing. The workflow otherwise derives the loopback dashboard
  upstream from resolved Stado config.
- DNS/TLS for `stado.wisent.com`; updater clients use only dedicated
  `stado://releases/...` URLs rendered through `/api/release/object`.

These are deliberately never auto-created by the coordinator; a missing item
must fail deployment rather than selecting GCP or creating a second writer.
