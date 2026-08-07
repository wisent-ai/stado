# GCP billing outage: blast radius and asset map

Date: 2026-07-27 UTC (2026-07-26 PDT)

Project: `wisent-480400`

Status: contained on a local-only Stado profile; GCP recovery is not verified because cloud access remains fenced and no post-restoration inventory is available.

## Scope and evidence quality

This report separates three evidence levels. They must not be merged into one apparent live inventory:

1. **Current observation** — read-only commands and local runtime logs during the incident.
2. **Last observed GCE snapshot** — a complete `gcloud compute instances list --format=json` capture written on 2026-07-13 03:39 PDT. This snapshot is stale and does not prove current state.
3. **Declared asset** — a named resource referenced by source code, deploy scripts, workflows, or migration records. It proves intended or historical use, not current existence.

Current live enumeration of GCS, Compute Engine, IAM, Secret Manager, Cloud Run revisions, Artifact Registry, Pub/Sub, BigQuery, and Cloud Build is unavailable. Therefore there is no defensible single number for all current GCP assets.

Incident-time and containment evidence are intentionally separated below. The
outage conclusions describe the configuration and behavior observed at failure
onset. A later local-only profile contains new work but does not prove recovery
of the inaccessible GCS state.

## Incident conclusion

A current `stado overview --json` request fails with:

```text
GCS API error HTTP 403
The billing account for the owning project is disabled in state absent
reason: accountDisabled
```

At incident onset, Stado had no config-file override. Compiled defaults and the LaunchAgent resolved to:

- project: `wisent-480400`;
- primary storage: GCS;
- primary bucket: `gs://stado`;
- compute providers: `gcp`;
- backup storage backend: empty;
- automatic failover: disabled.

At incident onset, the local `operator-host` agent started, initialized `JobStorage`, received the same GCS 403 before queue processing, exited, and was restarted by launchd roughly every 11–13 seconds.

Consequences:

- queue listing and mutation are unavailable;
- jobs cannot be reliably submitted, claimed, cancelled, completed, or inspected;
- status, results, leases, heartbeats, capacity, registry state, schedules, and billing-health state are unavailable;
- local workers are not a storage fallback because they also coordinate through GCS;
- there is no readable replica from which to establish current queue size or recovery point;
- there is no evidence that GCS objects were deleted, but the 403 also prevents proving that they remain present.

## Quantitative summary

### Incident-onset facts

| Measurement | Value | Evidence |
|---|---:|---|
| Readable primary queue stores | 0 | current `stado overview --json` |
| Configured backup stores | 0 | config defaults plus absent config file |
| Directly observed agents in crash-loop | 1 | local MacBook agent log |
| Exact current jobs | unknown | GCS listing blocked |
| Exact current VM, disk, bucket, or object count | unknown | provider APIs and GCS blocked |


### Current local containment snapshot

The current configuration explicitly enables only the `local` provider, fences
`gcp`, `azure`, and `aws`, stores state under `~/.stado/local-storage`, and uses
`~/.stado/local-backup` as a same-disk mirror. It explicitly warns that this is
not cross-provider disaster recovery.

The declared local control-plane surface contains 17 object namespaces with 123
allowed prefixes, 11 integration clients, 2 machine clients, 10 release
publishers, 3 managed-service deployers, and 15 agent secret items. These counts
measure the routed application surface, not recovered GCS objects.

Filesystem inspection found 179 payload files totaling 2,951,765,553 bytes in
the local primary and 200 payload files totaling 2,951,766,620 bytes in the
same-disk mirror. Most current payload is under `ecosystem/`; the primary has
only one `queue/` object and two `runs/` objects. This is post-incident local
state and is not evidence that the historical GCS queue, media, models, or
artifacts were recovered.

### Last observed Compute Engine snapshot

| Asset class | Count | Detail |
|---|---:|---|
| VM resources | 27 | 10 `RUNNING`, 1 `STAGING`, 16 `TERMINATED` |
| Active or staging VMs | 11 | 7 GPU VMs and 4 CPU-only VMs |
| GPU attachments across all 27 VMs | 23 | 11 T4, 6 A100 40 GB, 4 A100 80 GB, 2 L4 |
| GPU attachments on active/staging VMs | 7 | 3 A100 40 GB, 2 A100 80 GB, 2 L4 |
| Persistent boot disks attached to snapshot VMs | 27 | all snapshot attachments had `autoDelete=true` |
| Local SSD attachments | 6 | 375 GB each; no persistent source URI |
| Total attached disk capacity | 7,870 GB | 3,720 GB active/staging; 4,150 GB terminated |
| Observed zonal managed instance groups | 6 | derived from `created-by` metadata URLs |
| Observed instance-template revisions | 7 | derived from `instance-template` metadata |

The 27 VMs, 27 persistent disks, 6 Local SSD attachments, 6 observed MIG resources, and 7 template revisions form **73 enumerated compute objects in the stale snapshot**. This is not a current total and excludes detached disks, snapshots, reservations, images, addresses, load balancers, networks, firewall rules, Cloud Run, GCS, IAM, Secret Manager, BigQuery, Pub/Sub, Artifact Registry, and Cloud Build.

### Declared named non-VM resources

The source and deployment map below contains **53 distinct named non-VM resource identifiers**:

| Declared class | Named identifiers |
|---|---:|
| GCS buckets | 6 |
| Artifact Registry repositories | 5 |
| Cloud Run services or referenced service endpoints | 3 |
| Cloud Scheduler jobs | 1 |
| Pub/Sub topics | 2 |
| BigQuery datasets/tables | 2 |
| VPC networks | 1 |
| Static addresses | 1 |
| Firewall rules | 5 |
| Health checks | 3 |
| Backend services | 3 |
| Forwarding rules | 1 |
| Custom image families | 2 |
| Service accounts | 6 |
| Secret Manager secret names | 11 |
| Workload Identity Federation providers | 1 |

This is a count of names referenced by code or migration records, not a live GCP count. It includes resources recorded as migrated, legacy, optional, or referenced by an endpoint; billing-disabled APIs prevent confirming which 53 still exist. MIGs and instance templates are counted in the stale compute snapshot instead of being counted again here.

### Active Stado topology in the bundled registry

| Asset | Kind | Intended use |
|---|---|---|
| `local-control-plane` | local daemon coordinator | sole queue scheduler and Stado control plane, exposed through `stado.wisent.com` |
| `control-host` | local target | primary Weles browser worker; pinned placement |
| `gpu-host` | local GPU target | two RTX Pro 6000 slots for external/ComfyUI workloads |
| `operator-host` | local CPU target | two slots for CPU/TUI and Probierz remote jobs |

Cloud providers remain explicitly fenced. This topology is authoritative
declaration, not live heartbeat evidence.

## VM-by-VM usage map

Confidence meanings:

- `high`: exact VM/MIG name is referenced by the repository or matches the Stado naming contract;
- `medium`: workload and repository are supported by adjacent code and storage paths, but the exact VM name is not referenced;
- `low`: classification is based only on the resource name; retain for owner confirmation.

| VM | Snapshot state | Zone | Machine / GPU | Attached GB | Repository | Usage | Confidence |
|---|---:|---|---|---:|---|---|---|
| `image-gen-comfyui-mig-62fj` | RUNNING | `us-central1-a` | `g2-standard-8` / 1 L4 | 200 | `wisent-ai/echo-web` | ComfyUI/Z-Image gateway and fallback image generation | high |
| `vatt-a100` | TERMINATED | `us-central1-a` | `a2-highgpu-1g` / 1 A100 40 GB | 300 | unknown | VATT A100 experiment; no repository reference found | low |
| `wisent-agent-80gb-1777754741-3` | TERMINATED | `us-central1-a` | `a2-ultragpu-1g` / 1 A100 80 GB | 575 | `wisent-ai/wisent-compute` | ephemeral Stado/wisent-compute queue worker | high |
| `wisent-mig-inference-blue-wqj9` | RUNNING | `us-central1-a` | `a2-ultragpu-1g` / 1 A100 80 GB | 675 | `wisent-ai/wisent-backend` | GPU inference service in blue/green MIG | high |
| `wisent-1333bd59` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-25757579` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-5c56e71c` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-6fd1aaad` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-72c5dfcd` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-7c82d36c` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-82f03ff4` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-88eaf01f` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-bc5a7895` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-ed259745` | TERMINATED | `us-central1-b` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `wisent-mig-api-blue-j0qh` | RUNNING | `us-central1-b` | `e2-standard-2` | 50 | `wisent-ai/wisent-backend` | public API service in blue/green MIG | high |
| `wisent-mig-api-green-88dw` | RUNNING | `us-central1-b` | `e2-standard-2` | 50 | `wisent-ai/wisent-backend` | public API service in blue/green MIG | high |
| `wisent-mig-images-green-cq1d` | RUNNING | `us-central1-b` | `a2-highgpu-1g` / 1 A100 40 GB | 475 | `wisent-ai/wisent-backend` | GPU image service in blue/green MIG | high |
| `wisent-mig-images-green-vlrb` | RUNNING | `us-central1-b` | `a2-highgpu-1g` / 1 A100 40 GB | 475 | `wisent-ai/wisent-backend` | GPU image service in blue/green MIG | high |
| `wisent-mig-inference-blue-035m` | RUNNING | `us-central1-b` | `a2-highgpu-1g` / 1 A100 40 GB | 200 | `wisent-ai/wisent-backend` | GPU inference service in blue/green MIG | high |
| `wisent-zimage-interp-jt` | TERMINATED | `us-central1-b` | `a2-highgpu-1g` / 1 A100 40 GB | 300 | `wisent-ai/echo-web` | Z-Image interpolation experiment | medium |
| `needher-watermark-a100-80gb-20260622` | RUNNING | `us-central1-c` | `a2-ultragpu-1g` / 1 A100 80 GB | 575 | `echo-web` + `needher-ai-web` | Needher watermark/content pipeline | medium |
| `sapiens2-bodyhorror` | TERMINATED | `us-central1-c` | `a2-ultragpu-1g` / 1 A100 80 GB | 575 | `echo-web` + `needher-ai-web` | Sapiens2/Needher content or watermark experiment | low |
| `wisent-b5d3e0ee` | TERMINATED | `us-central1-f` | `n1-standard-4` / 1 T4 | 200 | `wisent-ai/wisent-compute` | legacy ephemeral T4 queue worker | high |
| `content-platform-vm` | RUNNING | `us-west1-b` | `n2-standard-16` | 500 | `wisent-ai/echo-web` | long-lived Echo Web/Weles worker and recordings host | high |
| `image-video-router-vm` | RUNNING | `us-west1-b` | `e2-small` | 20 | `wisent-ai/image-video-router` | long-lived image/video routing service | high |
| `wisent-agent-a100-1777737822-27` | TERMINATED | `us-east1-b` | `a2-highgpu-1g` / 1 A100 40 GB | 200 | `wisent-ai/wisent-compute` | ephemeral Stado/wisent-compute queue worker | high |
| `wisent-agent-l4-1783938808-1` | STAGING | `us-east4-a` | `g2-standard-4` / 1 L4 | 500 | `wisent-ai/wisent-compute` | ephemeral Stado/wisent-compute queue worker | high |

Group totals from this classification:

| Owner/workload | VM count | Snapshot active/staging | Notes |
|---|---:|---:|---|
| Stado/wisent-compute ephemeral fleet | 14 | 1 | 11 legacy `wisent-<hex>` plus 3 `wisent-agent-*` |
| Wisent backend MIG services | 6 | 6 | API 2, image 2, inference 2 |
| Content ComfyUI gateway | 1 | 1 | managed L4 worker |
| Standalone content/Needher experiments | 4 | 1 | three classifications require owner confirmation |
| Long-lived product CPU VMs | 2 | 2 | content platform and image/video router |

Configuration drift: the snapshot places `image-video-router-vm` in `us-west1-b`, while its current deploy script defaults to `us-central1-a`. Recovery must use the actual live zone, not the current script default.

## Managed compute assets observed through VM metadata

### Zonal managed instance groups

| Manager | Repository | Use |
|---|---|---|
| `us-central1-a/image-gen-comfyui-mig` | `echo-web` | L4 ComfyUI gateway; autoscaling declaration min 1, max 3 |
| `us-central1-a/wisent-mig-inference-blue` | `wisent-backend` | A100 80 GB inference |
| `us-central1-b/wisent-mig-api-blue` | `wisent-backend` | CPU API blue deployment |
| `us-central1-b/wisent-mig-api-green` | `wisent-backend` | CPU API green deployment |
| `us-central1-b/wisent-mig-images-green` | `wisent-backend` | A100 image service |
| `us-central1-b/wisent-mig-inference-blue` | `wisent-backend` | A100 inference |

The backend deployment also declares inactive blue/green companions such as `wisent-mig-images-blue` and `wisent-mig-inference-green`; their current existence and size are not verified.

### Instance-template revisions represented by snapshot VMs

- `image-gen-comfyui-template-20260306-135844`
- `wisent-it-api-blue-r152-1`
- `wisent-it-api-r241-1`
- `wisent-it-images-green-r32-1`
- `wisent-it-images-green-r35-1`
- `wisent-it-inference-blue-r31-1`
- `wisent-it-inference-blue-r36-1`

Two image-service revisions were simultaneously represented, consistent with a rolling update or an incomplete drain.

## GCS buckets and object usage

Six concrete bucket names appear in operational or migration code. Their current existence and object counts cannot be checked.

| Bucket | Main users | Data stored and operational role | Evidence |
|---|---|---|---|
| `gs://stado` | `wisent-compute`, `echo-web`, `wisent-tools`, `wisent-tester/probierz` | canonical queue, lifecycle records, capacity, schedules, config, artifacts, Probierz inputs/results | current default and deployment |
| `gs://wisent-compute` | `wisent-compute`, `compute.wisent.com`, `wisent-tools`, `OpenEnv` | legacy queue/registry, release binaries, agent binary/logs, host health, status output, Supabase token fallback | code and deployment |
| `gs://wisent-gcp-pipeline` | `echo-web`, `needher-ai-web`, `wisent-backend` | generated images, Needher watermarks and feed snapshots, LoRA training output, ComfyUI batch inputs/results, model staging | code |
| `gs://wisent-images-bucket` | `echo-web`, `wisent-backend`, `wisent` | SmoothMix models and videos, LoRAs, control vectors, optimization checkpoints | code |
| `gs://kantbench-training` | `OpenEnv` | training code, checkpoints, evaluation output, sweep state | code and session record |
| `gs://wisent-gcp-bucket` | `wisent-backend` | legacy S3-to-GCS mirror for control vectors, images, characters, models, and training | migration script; current use unverified |

Object-count records disagree and must not be treated as live:

- the migration plan describes the old `wisent-compute` queue as containing `480k+` files;
- the recorded copy to `stado` observed 169,073 source objects and 169,071 destination objects at that copy point;
- no current count is available for either bucket or for any content/training bucket.

## Artifact Registry and release assets

| Repository/image namespace | Consumers | Use |
|---|---|---|
| `us-central1-docker.pkg.dev/wisent-480400/stado` | `wisent-compute` | `stado-coordinator:<version>` Cloud Run image |
| `us-central1-docker.pkg.dev/wisent-480400/compute-backend` | `compute.wisent.com` | marketplace backend image |
| `us-central1-docker.pkg.dev/wisent-480400/image-video-router` | `image-video-router` | router service image |
| `us-central1-docker.pkg.dev/wisent-480400/wisent-backend` | `wisent-backend` | API, inference, image service, combined service, ASC bridge images |
| `us-central1-docker.pkg.dev/wisent-480400/smoothmix-comfyui` | `echo-web` | ComfyUI/SmoothMix GPU workload images |
| `gs://wisent-compute/releases/stado/<version>/linux-amd64` | all Stado hosts | released Rust binaries, checksums, and `latest.json` channel pointer |

Current repository/image counts and latest successful Cloud Builds are unknown.

## Control-plane, messaging, billing, and network assets

These are named resources declared by deploy code or previously recorded as created.

| Resource | Kind | Repository | Use/status note |
|---|---|---|---|
| `stado-coordinator` | Cloud Run | `wisent-compute` | continuous Rust scheduler; configured min=max=1 |
| `wisent-compute-backend` | Cloud Run/backend URL | `compute.wisent.com` | marketplace API/provisioner; exact deployment declaration not present in the local repo |
| `asc-webhook-bridge` | Cloud Run | `wisent-backend` | App Store Connect webhook to GitHub dispatch |
| `wisent-compute-cron` | Cloud Scheduler | `wisent-compute` | authenticated GET to coordinator `/livez` every 3 minutes |
| `wisent-compute-alerts` | Pub/Sub | `wisent-compute` | alert topic created and explicitly injected into current coordinator deploy |
| `stado-alerts` | Pub/Sub | `wisent-compute` | renamed topic recorded as created; Rust config default when no env override |
| `billing_export` | BigQuery dataset | `wisent-compute` | GCP billing export read by billing-health collector |
| `gcp_billing_export_v1_017364_D3B657_F207B5` | BigQuery table | `wisent-compute` | gross cost, credits, net cost, and burn history |
| `default` | VPC network | content and backend compute | common VM, firewall, and internal-LB network |
| `comfyui-static-ip` | regional static address | `echo-web` | stable external ComfyUI endpoint |
| `allow-comfyui-8188` | firewall rule | `echo-web` | public and health-check access to ComfyUI port 8188 |
| `image-gen-comfyui-health` | health check | `echo-web` | `/system_stats` auto-healing check |
| `wisent-api-hc` | health check | `wisent-backend` | API `/health` |
| `wisent-inference-hc` | regional health check | `wisent-backend` | inference `/health` on port 8001 |
| `wisent-allow-api-health` | firewall rule | `wisent-backend` | health-check access to API |
| `wisent-allow-api-lb` | firewall rule | `wisent-backend` | load-balancer access to API |
| `wisent-allow-inference-hc` | firewall rule | `wisent-backend` | health-check access to inference |
| `wisent-allow-inference-internal` | firewall rule | `wisent-backend` | VPC access to inference |
| `wisent-bs-api-blue` | global backend service | `wisent-backend` | API blue backend |
| `wisent-bs-api-green` | global backend service | `wisent-backend` | API green backend |
| `wisent-inference-internal-bs` | regional backend service | `wisent-backend` | internal inference backend |
| `wisent-inference-internal-fwd` | forwarding rule | `wisent-backend` | stable internal inference VIP on port 8001 |
| `image-gen-comfyui` | custom image family | `echo-web` | baked ComfyUI, Z-Image, models, and LoRAs |
| `wisent-agent` | custom image family | `wisent-compute` | baked Stado cloud-agent VM image |

The retired `gcp_setup.sh` path used `wisent-compute-alerts`, while the Rust
default is `stado-alerts`. That direct cloud-CLI provisioning path has been
removed; any explicitly enabled GCP adapter now consumes only pre-provisioned
resources named in the authoritative deployment profile.

The legacy Gen2 function `wisent-compute-tick` is explicitly deleted during Rust coordinator deployment. Treat it as retired unless live inventory proves otherwise.

## IAM and Secret Manager asset map

### Service accounts

| Service account | Consumers | Use |
|---|---|---|
| `wisent-compute-sa@wisent-480400.iam.gserviceaccount.com` | `wisent-compute` | coordinator, scheduler OIDC, GCS, Compute, Pub/Sub, Secret Manager, and BigQuery access |
| `stado-sa@wisent-480400.iam.gserviceaccount.com` | `wisent-compute` migration | renamed agent identity recorded as created; current coordinator still deploys with `wisent-compute-sa` |
| `droid-441@wisent-480400.iam.gserviceaccount.com` | `wisent-backend`, `echo-web` | backend MIG templates and ComfyUI template |
| `wisent-images-sa@wisent-480400.iam.gserviceaccount.com` | `wisent-backend` image service | objectAdmin on image bucket and objectViewer on pipeline bucket |
| `kantbench-training@wisent-480400.iam.gserviceaccount.com` | `OpenEnv` | training VM, GCS, Secret Manager, and Vertex AI access |
| `asc-webhook-bridge@wisent-480400.iam.gserviceaccount.com` | `wisent-backend` | Cloud Run runtime identity for ASC webhook bridge |

GitHub workflows also reference a Workload Identity Federation provider under `github-pool/providers/github-provider`; its current IAM policy is not enumerated.

### Named secrets

| Secret | Consumers | Use/status note |
|---|---|---|
| `wisent-hf-token` | `wisent-compute` | HF token injected into coordinator |
| `wisent-gh-token` | `wisent-compute` | GitHub access for agents/bootstrap |
| `wisent-azure-billing-sp` | `wisent-compute` | optional cross-cloud billing principal referenced by legacy setup comments; current value may instead live in Skarbiec |
| `wisent-api-env` | `wisent-backend` | API runtime environment |
| `wisent-images-env` | `wisent-backend` | image-service runtime environment |
| `wisent-inference-env-bf16-a10080` | `wisent-backend` | current inference runtime environment in deploy workflow |
| `wisent-inference-env` | `wisent-backend` | older/default inference secret name retained by setup script |
| `asc-webhook-secret` | `wisent-backend` | ASC request authentication |
| `github-dispatch-token` | `wisent-backend` | GitHub workflow dispatch from ASC bridge |
| `wandb-api-key` | `OpenEnv` | experiment tracking |
| `hf-token` | `OpenEnv` | Hugging Face training/download access |

Secret existence, versions, IAM bindings, and rotation state are not currently verified. Secret values were not read.

## Repository blast radius

### Direct runtime dependencies

| Repository | Dependency | Incident effect |
|---|---|---|
| `wisent-ai/wisent-compute` | Stado queue, coordinator, registry, workers, provisioning, releases | critical control-plane outage |
| `wisent-ai/echo-web` | `wc submit`, GCS output/DONE markers, ComfyUI gateway, GCP image assets | new renders and their status/artifacts unavailable |
| `wisent-ai/wisent-tools` | direct `stado.queue.submit`, `JobStorage`, coverage plugin, GCS activation data | submission, deduplication, status, and activation pipelines unavailable |
| `lbartoszcze/wisent-tester` (`probierz`) | `gs://stado/probierz-inputs`, `wc submit`, `probierz-results` | remote run/author flows unavailable; local-only flows remain usable |

### Shared-project or artifact dependencies

| Repository | Dependency | Incident effect |
|---|---|---|
| `wisent-ai/wisent-backend` | six observed MIG VMs, load balancers, Artifact Registry, GCS, secrets | existing workloads may remain process-alive, but management, replacement, secrets, and storage are at risk |
| `wisent-ai/compute.wisent.com` | direct GCE provisioning, Artifact Registry, `gs://wisent-compute/agent` | separate from the Stado queue but inside the same GCP billing failure domain |
| `wisent-ai/OpenEnv` | `kantbench-training`, `wisent-compute` status output, training SA/secrets | GCP training, recovery, logs, checkpoints, and evaluation retrieval unavailable |
| `wisent-ai/image-video-router` | long-lived GCE VM and Artifact Registry | service may continue while its VM runs; redeploy and replacement are at risk |
| `wisent-ai/needher-ai-web` | `wisent-gcp-pipeline` generated and watermarked content | content backfills, snapshots, and poster ingestion dependent on GCS are unavailable |
| `wisent-ai/wisent` | optional Stado HF rate limiter and `wisent-images-bucket` checkpoints | core local library is not globally down; GCP-backed coordination/checkpoints are affected |

Local clones and worktrees are deduplicated by Git remote. For example, local `echo` and `echo-production` both point at `wisent-ai/echo`, and `echo-web` is the repository formerly named `content-platform`; neither pair is a separate production repository.

## Recovery inventory order

Once billing is restored, collect a fresh read-only inventory before mutating resources:

1. project lifecycle and billing link;
2. all GCS buckets, locations, object counts, bytes, retention, versioning, and IAM;
3. VM, MIG, disk, snapshot, reservation, image, address, network, firewall, health-check, backend-service, and forwarding-rule inventory;
4. Cloud Run services and revisions, Cloud Scheduler jobs, and retired Cloud Functions;
5. Artifact Registry repositories/images and latest Cloud Builds;
6. service accounts, project roles, workload-identity bindings, and `testIamPermissions` results;
7. Secret Manager metadata and access policy without reading secret values;
8. Pub/Sub topics/subscriptions and BigQuery billing-export dataset/table;
9. primary `gs://stado` versus any backup namespace coverage;
10. owner confirmation for `vatt-a100`, `wisent-zimage-interp-jt`, `needher-watermark-a100-80gb-20260622`, and `sapiens2-bodyhorror`.

Do not delete terminated instances, disks, templates, MIGs, or legacy buckets until the fresh inventory proves ownership, replacement, and recovery requirements.
