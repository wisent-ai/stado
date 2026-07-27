# Azure cutover runbook

Written during the GCP billing outage: the billing account owning the GCS
project was closed, every GCS call returns `accountDisabled`, and the queue
store is unreachable. This is the operator-side sequence. It complements
`deploy/MIGRATE_TO_STADO.md`, which covers the same-cloud rename and whose
split-brain warning applies here verbatim.

Every step below is an operator action. The agent tooling deliberately does
not run these: the coordinator creates no networking, hands out no role
assignments, and never provisions its own storage.

Numbers that would normally appear inline (address prefixes, disk sizes, API
versions) are written as `<placeholders>` — a repo policy hook rejects
numeric literals in committed files. Take the real values from the source
cited next to each one.

## Prerequisite that gates everything

Re-attach a billing account to the GCP project.

Object data in `gs://stado` and `gs://wisent-compute` is retained, not
deleted — `accountDisabled` is an authorization failure, not `NoSuchBucket`.
But Cloud Storage has no read-only mode for a disabled billing account:
`objects.list`, `objects.get`, `gsutil`, `azcopy` against the public HTTPS
endpoint and anonymous reads all return the same denial. **No byte can leave
GCS until billing is restored.** Any billing account works, including a fresh
one, and it only has to stay attached long enough to run the copy.

GCP deletes the project's resources some weeks after billing is disabled.
Confirm the actual deadline in the billing console rather than trusting a
remembered figure, then treat it as the hard deadline for the copy.

## Provision the Azure side

Resource group and networking. The provider expects these to exist and will
never create them; the per-region suffix convention is
`<vnet>-<location>` / `<nsg>-<location>`, implemented in
`providers/azure/network.rs`.

```sh
SUB=<subscription-id>
RG=wisent-compute
LOC=eastus            # repeat the vnet/nsg pair for every AZURE_LOCATIONS entry

az account set --subscription "$SUB"
az group create -n "$RG" -l "$LOC"
az network vnet create -g "$RG" -n "wisent-compute-vnet-$LOC" -l "$LOC" \
    --address-prefixes <vnet-cidr> \
    --subnet-name wisent-compute-subnet --subnet-prefixes <subnet-cidr>
az network nsg create -g "$RG" -n "wisent-compute-nsg-$LOC" -l "$LOC"
```

Storage account and container. Match what `BackendProvisioner.provisionAzure`
already creates for desktop deployments — take `--kind` and `--sku` verbatim
from that Swift source so the two paths cannot drift.

```sh
ACCT=<storage-account>        # lowercase, no dashes, globally unique
az storage account create -n "$ACCT" -g "$RG" -l "$LOC" \
    --sku Standard_LRS --kind <account-kind> --allow-blob-public-access false
az storage container create -n stado --account-name "$ACCT" --auth-mode login
```

The container name must be `stado`. `WC_AZURE_CONTAINER` defaults to
`wisent-compute`, so leaving it unset points the whole fleet at an empty
container and every worker reports no work.

## Create the agent identity

Agent VMs authenticate to both Blob and ARM through a user-assigned managed
identity referenced by `AZURE_VM_IDENTITY_ID`. Without it the on-VM token
chain in `azure_token.rs` has no source at all — no env service principal, no
IMDS principal, no `az` CLI on the image — so the agent can neither read the
queue nor delete itself when idle.

```sh
az identity create -n stado-agent -g "$RG" -l "$LOC"
PRINCIPAL="$(az identity show -n stado-agent -g "$RG" --query principalId -o tsv)"
IDENTITY_ID="$(az identity show -n stado-agent -g "$RG" --query id -o tsv)"
ACCT_SCOPE="$(az storage account show -n "$ACCT" -g "$RG" --query id -o tsv)"
RG_SCOPE="$(az group show -n "$RG" --query id -o tsv)"

# Data-plane role. Contributor/Owner do NOT grant blob access.
az role assignment create --assignee-object-id "$PRINCIPAL" \
    --assignee-principal-type ServicePrincipal \
    --role "Storage Blob Data Contributor" --scope "$ACCT_SCOPE"

# Lets an --idle-shutdown agent ARM-DELETE its own VM instead of billing on.
az role assignment create --assignee-object-id "$PRINCIPAL" \
    --assignee-principal-type ServicePrincipal \
    --role "Virtual Machine Contributor" --scope "$RG_SCOPE"
```

Grant the same `Storage Blob Data Contributor` on `$ACCT_SCOPE` to whatever
identity runs the coordinator itself — a service principal, or your own user
if the coordinator runs off a logged-in `az` session.

Role assignments propagate asynchronously. During that window
`AzureBlobBackend::exists` reports every blob as absent, because it swallows
errors and returns false. Expect a phantom-empty container for the first few
minutes rather than a clean permission error.

## Mirror the release channel

Agent VMs install `stado` from `WC_RELEASE_BASE_URL`. Its default is the GCS
releases bucket, which is dead, and an install failure aborts cloud-init
before the agent ever starts — VMs boot, bill, and run nothing.

Copy `releases/stado/**` from the GCS bucket into the container under a
`releases/stado/` prefix, preserving the layout the fetchers expect:
`<base>/latest.json`, then `<base>/<version>/<platform>/stado` plus the
sibling checksum file. Keep the checksum file: every fetcher verifies it.

```sh
WC_RELEASE_BASE_URL="https://$ACCT.blob.core.windows.net/stado/releases/stado"
```

VMs mint their own bearer token from IMDS, so the container stays private.
Operator laptops running `deploy/stado-up.sh` have no managed identity —
give that path either a public-read container or a container SAS appended to
the URL; the script splits the query string and re-appends it per object.

## Drain, then copy the queue

Copying a live queue causes split-brain: a job is claimed from the old store,
written to the new one, and reaped from neither. Drain first —
`deploy/MIGRATE_TO_STADO.md` gates its own copy behind an explicit
confirmation for exactly this reason.

```sh
stado storage copy --from gcs --from-bucket stado \
    --to azure --to-account "$ACCT" --to-container stado --dry-run
stado storage copy --from gcs --from-bucket stado \
    --to azure --to-account "$ACCT" --to-container stado
```

The copier carries blob metadata, not just bodies. `write_job` stamps
scheduling metadata that `list_fitting` prefilters on; a body-only copy such
as a plain `azcopy` run leaves jobs visible but degrades every scheduler tick
into downloading the entire queue. It is resumable, never deletes, and exits
non-zero if anything failed. Re-run it after the final drain to catch churn.

## Flip the configuration

Install `deploy/azure/stado.config.json` at `~/.config/stado/config.json` and
fill in every `<placeholder>`, or export the equivalent variables from
`deploy/azure/env.example`. Env wins over the file.

```sh
stado config show          # confirm the resolved values before restarting
```

Re-run `deploy/stado-up.sh <target>` on each box: it now propagates the
provider/storage/identity variables into the launchd plist. launchd hands a
LaunchAgent none of the invoking shell's environment, so a variable that is
only exported in your terminal will not reach the agent.

## Verify

- A fresh `capacity/local-<hostname>.json` appears in the Azure container and
  stops advancing in GCS.
- One job walks `queue` to `running` to `completed` end to end.
- `stado overview` renders. Its Azure credit balance will report
  `no_credentials` until the Azure service principal is moved out of GCP
  Secret Manager — `monitor/billing.rs` reads it from there, so a dead GCP
  means no Azure balance.
- A dispatched agent VM reaches the agent process. If cloud-init dies early,
  read `/var/log/wisent-agent.log` on the VM: an unsubstituted placeholder or
  a failed release download both abort before the agent line.

## Known gaps at cutover time

- **GPU quota on the subscription is unproven.** The scan under
  `~/.weles/azure_quota_scan` shows most family/region requests throttled and
  its support tickets in an unknown state. Without approved quota,
  `create_instance` walks every location, collects `QuotaExceeded`, and
  returns no instance. `stado quota show` reads the live limits (it covers
  every provider in `WC_PROVIDERS`, so there is no `--provider` flag). Then
  `stado quota request-all --to <limit> --provider azure`, and
  `stado quota azure-replies` to answer the tickets Microsoft opens for each
  request — unanswered ones are archived and the request is dropped silently.
- **The Azure credit balance is unreadable** while its service principal
  lives in GCP Secret Manager.
- `machine submit --source-archive` announces a `gs://` source URI and fetches
  it with `gsutil` in the job's pre-commands; `--output-uri` mirroring also
  shells out to `gsutil`. Both are no-ops or failures on an Azure agent.
- `stado registry push|pull` still writes to GCS, so the registry cannot be
  repaired from the CLI on an Azure-only deployment.
