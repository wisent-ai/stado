# GCP service and consumer inventory — 2026-08-15

## Scope and measurement

Project: `wisent-480400` (`1080673333190`). Billing is intentionally detached and must remain detached.

A live Cloud Asset Inventory read returned **1,057 assets**:

| Layer | Count | Interpretation |
|---|---:|---|
| Compute Engine assets | 599 | VMs, disks, MIGs, templates, images, load balancers, VPC resources and generated regional settings |
| Non-compute logical/support resources | 109 | buckets, identities, secrets, Cloud Run, Cloud SQL, BigQuery, Pub/Sub, monitoring and supporting resources listed below |
| Enabled API records | 86 | an API being enabled is not evidence that a workload still uses it |
| Quota preferences | 119 | mainly historical GPU and regional capacity requests, not running services |
| Secret versions | 132 | history under 19 Secret Manager secrets; values were not read |
| Service-account keys | 12 | keys attached to eight of the fourteen user-managed service accounts |

The historical compute workload and disk inventory remains in `docs/gcp-compute-retirement-2026-08-15.json`. This document adds the rest of the project and distinguishes a deployed workload from its support records and from an API that is merely enabled. The unapproved assistant assessment of resource necessity is in [`gcp-resource-need-map-2026-08-15.md`](gcp-resource-need-map-2026-08-15.md).

## Product workload map

| Capability formerly placed in GCP | GCP resources | Producers / consumers | Current placement or status |
|---|---|---|---|
| Stado control plane and queue | `gs://stado`, legacy `gs://wisent-compute`, historical `stado-coordinator`, `wisent-compute-cron`, `stado-alerts`, `wisent-compute-alerts`, billing BigQuery, Stado service accounts and monitoring policies | `wisent-compute`; jobs from Echo, `wisent-tools`, OpenEnv and Probierz | Coordinator and object/release APIs moved to `charless-mac-mini`; Azure Blob is primary object storage and S3 is DR. GCP Cloud Run/Scheduler resources are absent from the live asset inventory; both GCS namespaces remain. |
| Wisent Backend public API | API blue/green MIGs, backend services, health checks, load balancer, Artifact Registry image, `wisent-api-env`, `droid-441` | Wisent apps and product clients | Runtime moved to `charless-mac-mini`. GCP instances are absent, but MIG/LB/template/image support assets remain. |
| Wisent Backend model inference | inference blue/green MIGs across `us-central1-a/b/c`, internal backend/VIP/health check, inference env secrets | Wisent Backend; serving requests now belong through Brama | Target is Brama plus Stado `chat-primary` on `ubuntu-server-rtx-pro-6000`; target was documented as not fully online. GCP support assets remain without live instances. |
| Wisent Backend image generation | image blue/green and regional MIGs, image backends/CDN/VPC, `wisent-images-*` secrets, `wisent-images-sa`, image/model buckets | character generation, companion/entertainment scenes, NeedHer pictures/poses and chat image tools through Wisent Backend | Target is `wisent-backend-images` on local GPU 1, but no canonical Stado service endpoint currently wires the path end to end. |
| ComfyUI and Z-Image workflows | `image-gen-comfyui-mig`, `comfyui-static-ip`, gateway/firewall/health assets, custom images/templates, `gs://wisent-gcp-pipeline` | Echo/content generation, NeedHer, Wisent Backend and caller-built workflows | Target is local ComfyUI on GPU 2. `image-video-router` may submit raw workflows; high-level direct image routing remains a separate contract. |
| General training / label-model | training blue/green MIGs and backends, Stado agent images/templates, `gs://kantbench-training`, HF/W&B secrets, KantBench service account | OpenEnv/KantBench, transcript label training and historical experiments | Target is local GPU 3 for one exclusive job and ephemeral Azure A100 capacity for overflow. Non-label historical training is not proven fully migrated. |
| Ephemeral Stado GPU workers | 14 historical T4/L4/A100 workers, templates/images, `gs://stado` queue and `gs://wisent-compute` legacy queue | Stado scheduler and queued workloads | Individual GCP workers are not required; local RTX capacity is primary and Azure is cloud burst. Twenty residual instances are terminated and their disks remain. |
| Echo/Weles content worker and recording host | `content-platform-vm`, 500 GB disk, content buckets | Echo, Weles and recording workflows | Durable duties moved to Echo/Weles on `charless-mac-mini`; unique recording data on the GCP disk is still retained for export. |
| Weles Apple authentication | `weles-apple-auth-prod`, 30 GB disk | Weles Apple sign-in flow | Replaced by Weles on `charless-mac-mini`; residual VM/disk are inert. |
| Image/video routing | `image-video-router-vm`, 20 GB disk, firewall and historical container namespace | Content Platform and approved Weles media workflows | Declared replacement is `image-video-router` on `ubuntu-server-rtx-pro-6000`; previously recorded as offline. |
| Swiatowid PTY relay | Cloud Run `swiatowid-pty-relay`, seven revisions, five current source-deploy image records, source bucket, relay token | No observed consumer | This is the only Cloud Run service resource in the live asset inventory. Its control-plane status is `Ready`, but request logs contain no requests in the last 90 days and runtime logs show repeated startup aborts because detached billing prevents the Secret Manager read. Current Oko source implements a Stado-managed relay contract, but this Mac has no configured relay URL; the GCP deployment is not a functioning or observed implementation. |
| Body-horror detection | Cloud Tasks queue `bodyhorror-detector`, `bodyhorror-detector` service account and `gs://wisent-body-horror-models` | Historical image-quality/body-horror pipeline | Queue asset says RUNNING, but no Cloud Run/Function detector exists and no current source reference to the queue was found. Treat as an orphaned legacy queue, not a working service. |
| Compute/marketplace database | Cloud SQL `wisent-compute-db`, PostgreSQL 15, 10 GB | Legacy `wisent-compute`/compute marketplace persistence; exact schema caller is not recoverable while stopped | `STOPPED`, activation policy `NEVER`, suspended for `BILLING_ISSUE`. Current Stado state is object-backed; the experimental marketplace is not operated. |
| Billing analytics | BigQuery dataset `billing_export` and table `gcp_billing_export_v1_017364_D3B657_F207B5` | Stado billing-health and agent billing reader | Historical GCP gross cost, credits, net cost and burn history; retained as data, not a runtime dependency. |
| App Store Connect webhook bridge | historical Cloud Run `asc-webhook-bridge`, service account, webhook secret and GitHub dispatch token | Wisent Backend release automation | No Cloud Run service is present now; identity and secrets remain. |
| Google Play / RevenueCat integration | topic `Play-Store-Notifications`, RevenueCat service account and two account keys | RevenueCat / Android publisher integration | No Pub/Sub subscription exists in this project; topic and identity remain. |
| Desktop updates | `gs://wisent-swiatowid-updates`, `gs://wisent-oko-updates` | Installed Swiatowid/Oko Sparkle clients | Historical release archives and `appcast.xml`; Oko release delivery now uses Stado release objects and GitHub release assets. |
| Stock context archive | `gs://wisent-stock-context` | Historical stock research/content workflow; prefixes exist for AAPL, ADBE, NOK, NVDA and ORCL | Data archive remains; an exact current producer is not present in the current source map. |
| One-off image/video/research work | `gs://wisent-video-gen`, experiment disks, model and pipeline prefixes | Civitai exact video run, VATT, Z-Image interpolation, NeedHer watermark and Sapiens2 work | Not permanent services. Unique disks/results remain retained where export has not been proven. |

## Cloud Storage: all 17 buckets

| Bucket | What it contains | Known users / status |
|---|---|---|
| `wisent-480400-skarbiec-vault` | `skarbiec.vault.json` | Legacy Skarbiec vault backup. Current Skarbiec uses its host-local encrypted vault and synchronization path; no current code points to this bucket. |
| `stado` | registry, queue, lifecycle state, machine requests, schedules, artifacts, logs, checkpoints, Probierz inputs/results | Canonical historical Stado object namespace used by `wisent-compute`, Echo, `wisent-tools` and Probierz; replaced as primary storage by Azure Blob with S3 DR. |
| `wisent-oko-updates` | Oko/Swiatowid archives and Sparkle `appcast.xml` | Historical Oko update feed; replaced by Stado/GitHub release delivery. |
| `wisent-swiatowid-updates` | Swiatowid archives and Sparkle `appcast.xml` | Older Swiatowid update feed; retained for old clients, no new release writer. |
| `wisent-body-horror-models` | `sapiens2_host/` model material | Historical body-horror detector. |
| `run-sources-wisent-480400-europe-west1` | Cloud Run source bundle under `services/swiatowid-pty-relay/` | Staging for the unused/nonfunctional relay; canonical relay source exists in Oko. |
| `wisent-stock-context` | per-symbol AAPL/ADBE/NOK/NVDA/ORCL context | Historical stock analysis/content archive; exact current producer unproven. |
| `wisent-compute` | legacy registry/queue, releases, agents, logs, status, schedules and run records | Legacy `wisent-compute`/Stado namespace and compute marketplace support. |
| `gcf-v2-sources-1080673333190-us-central1` | currently empty at top level | Generated Cloud Functions v2 source staging; no live Function asset. |
| `gcf-v2-uploads-1080673333190.us-central1.cloudfunctions.appspot.com` | currently empty at top level | Generated Cloud Functions v2 upload staging; no live Function asset. |
| `wisent-jobs-wisent-480400` | currently empty at top level | Legacy job bucket; no current consumer evidence. |
| `kantbench-training` | code, checkpoints, eval, hyperopt, Optuna and scripts | OpenEnv/KantBench training and evaluation. |
| `wisent-images-bucket` | generated images/video, LoRAs, characters, checkpoints, body-horror datasets, activations and control vectors | Echo, Wisent Backend image service and historical `wisent` experiments. |
| `wisent-video-gen` | `civitai-exact-153914/` | One-off image-to-video generation output. |
| `wisent-480400_cloudbuild` | `source/` | Legacy Cloud Build source staging. |
| `wisent-gcp-bucket` | legacy mirror of images, characters, models, activations, control vectors, enterprise jobs and training material | Wisent Backend migration/archive bucket; current use unverified. |
| `wisent-gcp-pipeline` | ComfyUI models and outputs, LoRAs, image/video runs, NeedHer watermarks/captions and SmoothMix | Echo, NeedHer, Wisent Backend and ComfyUI batch workflows. |

## Managed non-compute resources

### Cloud Run and Artifact Registry

- Deployed Cloud Run service resource: `swiatowid-pty-relay` in `europe-west1`; the control plane reports `Ready`, but this is stale with respect to runtime viability.
- Request-log query for the last 90 days returned zero entries.
- Runtime logs on 2026-08-11 show repeated minimum-instance starts aborting because `swiatowid-pty-relay-token` cannot be fetched while billing is detached.
- Seven relay revisions remain; latest is `swiatowid-pty-relay-00007-ctx`.
- Current Cloud Asset records contain six Docker images: five relay source-deploy revisions and one `wisent-backend/api-service` digest.
- The only repository resource still returned is `europe-west1/cloud-run-source-deploy`.
- Historical deploy code names `stado`, `compute-backend`, `image-video-router`, `wisent-backend` and `smoothmix-comfyui` image namespaces; those repository resources are not present in the live Cloud Asset repository list.
- Artifact Registry detail listing is blocked by `BILLING_DISABLED`; billing was not linked.

### Data and messaging

| Resource | Consumer / use | Current fact |
|---|---|---|
| BigQuery `billing_export` | Stado billing-health | Dataset present. |
| BigQuery `gcp_billing_export_v1_017364_D3B657_F207B5` | cost, credit and burn calculations | Table present. |
| Pub/Sub `stado-alerts` | Rust Stado default alert topic | Topic present; no subscriptions in the project. |
| Pub/Sub `wisent-compute-alerts` | retired setup/deploy alert topic | Topic present; no subscriptions. |
| Pub/Sub `wisent-job-alerts` | historical job monitor | Topic present; no subscriptions. |
| Pub/Sub `Play-Store-Notifications` | RevenueCat/Google Play notifications | Topic present; no subscriptions in this project. |
| Cloud Tasks `bodyhorror-detector` | old detector enqueue path | Queue present; detector runtime absent. |
| Cloud SQL `wisent-compute-db` | old compute persistence | Stopped and billing-suspended. |
| Storage Transfer `ios-qwen3-hf-restore-20260616152940` | iOS Qwen3/Hugging Face restore | Job record present; details blocked by detached billing. |
| Storage Transfer `12096763445881479088` | unknown legacy transfer | Job record present; source/destination unavailable through the blocked detail API. |
| Storage Transfer `11389745504699589352` | unknown legacy transfer | Job record present; source/destination unavailable through the blocked detail API. |

### API keys

| Display name | Count | Intended API / consumer evidence |
|---|---:|---|
| `translate-i18n` | 1 | Restricted to `translate.googleapis.com`; legacy localization tooling. |
| `gemini-image-gen` | 2 | Legacy direct Gemini image generation. Both key metadata records have no API restriction; exact caller is not encoded in the asset. Wisent-owned model access now belongs through Brama, not direct provider keys. |

No key value was read.

## IAM: all 14 user-managed service accounts

| Service account | Historical consumer / role | User-managed key assets |
|---|---|---:|
| `brama-runtime@wisent-480400.iam.gserviceaccount.com` | historical Brama runtime with Skarbiec | 0 |
| `stado-sa@wisent-480400.iam.gserviceaccount.com` | renamed Stado compute identity; Compute, Run, Scheduler, Pub/Sub, Secret Manager and billing reads | 0 |
| `asc-webhook-bridge@wisent-480400.iam.gserviceaccount.com` | App Store Connect bridge | 0 |
| `bodyhorror-detector@wisent-480400.iam.gserviceaccount.com` | Cloud Tasks enqueue and body-horror model-object access | 0 |
| `claude-pr-reviewer@wisent-480400.iam.gserviceaccount.com` | old Claude PR reviewer for Supabase schema repos; Vertex AI user | 1 |
| `wisent-compute-sa@wisent-480400.iam.gserviceaccount.com` | old Stado coordinator, scheduler, fleet, storage, alerts, secrets and billing | 1 |
| `wisent-monitor@wisent-480400.iam.gserviceaccount.com` | job monitor, Compute, Pub/Sub and storage | 0 |
| `kantbench-training@wisent-480400.iam.gserviceaccount.com` | OpenEnv/KantBench training, Vertex AI, GCS and secrets | 0 |
| `revenuecat-service-account@wisent-480400.iam.gserviceaccount.com` | RevenueCat / Play Store Pub/Sub integration | 2 |
| `wisent-images-sa@wisent-480400.iam.gserviceaccount.com` | Wisent Backend image service, Artifact Registry and image buckets | 1 |
| `wisent-480400@appspot.gserviceaccount.com` | App Engine default residual identity | 1 |
| `agent-billing@wisent-480400.iam.gserviceaccount.com` | BigQuery billing reader | 2 |
| `1080673333190-compute@developer.gserviceaccount.com` | default VM/Cloud Run identity configured on the nonfunctional PTY relay | 1 |
| `droid-441@wisent-480400.iam.gserviceaccount.com` | broad historical deployer for backend, image and relay resources; currently project Owner | 3 |

Workload Identity Federation also remains:

- pool `github-pool`;
- provider `github-provider`;
- issuer `https://token.actions.githubusercontent.com`;
- condition `assertion.repository_owner == 'wisent-ai'`;
- mappings for subject, repository and actor.

This provider can identify workflows from Wisent repositories, but the pool alone does not prove which current workflow still exchanges a token.

## Secret Manager: all 19 secrets and 132 versions

| Secret | Versions | Historical consumer / use |
|---|---:|---|
| `account-api-env` | 10 | legacy account API runtime |
| `asc-webhook-secret` | 1 | App Store Connect webhook authentication |
| `brama-skarbiec-gpg-private-key` | 1 | historical GCP Brama-to-Skarbiec runtime |
| `github-dispatch-token` | 1 | ASC bridge dispatch into GitHub |
| `hf-token` | 1 | OpenEnv/KantBench Hugging Face access |
| `supabase-access-token` | 1 | deployment/automation access to Supabase |
| `swiatowid-pty-relay-token` | 2 | configured on the nonfunctional Cloud Run PTY relay; runtime fetch fails with billing detached |
| `vast-api-key` | 1 | historical external GPU marketplace/provider access |
| `wandb-api-key` | 1 | training experiment tracking |
| `wisent-api-env` | 92 | Wisent Backend API environment |
| `wisent-gh-token` | 1 | Stado worker/bootstrap GitHub access |
| `wisent-hf-token` | 2 | Stado worker/coordinator Hugging Face access |
| `wisent-images-env` | 6 | Wisent Backend image service environment |
| `wisent-images-supabase-key` | 1 | image-service Supabase client |
| `wisent-images-supabase-service-role-key` | 1 | image-service privileged Supabase client |
| `wisent-images-supabase-url` | 1 | image-service Supabase endpoint |
| `wisent-inference-env` | 4 | older/default inference environment |
| `wisent-inference-env-bf16-a10080` | 4 | A100 80 GB BF16 inference environment |
| `wisent-local-bmc-redfish` | 1 | local workstation BMC/Redfish control credential stored in the old cloud secret plane |

Secret values were not read. Current Wisent secret ownership belongs to Skarbiec; these records identify old consumers and migration surface, not an approved new runtime dependency on GCP.

## Logging, monitoring and administrative support

| Resource family | Names | What used it |
|---|---|---|
| Log metrics | `wisent_never_worked_reaps`, `wisent_tick_scheduled_zero`, `wisent_reap_events`, `wisent_tick_errors` | Old Stado/GCP scheduler and worker lifecycle monitoring. |
| Alert policies | never-worked reap, queue not draining, dead-agent reap spike, tick error rate | Old Stado coordinator alerts. |
| Notification channel | `wisent-compute-alerts-email` | Alert delivery; verification state is unspecified. |
| Log buckets/sinks | `_Required`, `_Default` | Project audit and default logs; Google-managed support resources. |
| Dataplex entry groups | ten automatic groups for Cloud SQL, BigQuery, Pub/Sub, Storage, Vertex AI, Bigtable, Spanner, Analytics Hub and Dataproc Metastore | Metadata discovery records, not ten deployed Wisent services. |
| Essential Contact | contact `1` | Project administrative notification contact. |
| Org Policy | `iam.allowedPolicyMemberDomains` | IAM domain restriction. |
| Connectivity Test | `ssh-troubleshoot-cmh25` | One historical `gcloud compute ssh --troubleshoot` diagnostic; not a runtime path. |
| Billing info | project billing record | State `CLOSED`; billing intentionally detached. |
| Quota preferences | 119 | Historical GPU/CPU/regional quota requests. They reserve no workload and prove no running capacity. |

## Compute support estate grouped by actual service

The exact 599-resource compute inventory is counted in `docs/gcp-compute-retirement-2026-08-15.json`. The service mapping is:

| Family | Named assets and counts | Consumer |
|---|---|---|
| Backend API | `wisent-mig-api-blue/green`, `wisent-bs-api*`, `wisent-api-hc`, `wisent-lb-ip`, HTTP/HTTPS forwarding, URL maps, proxies and `wisent-ssl-cert-v2` | Wisent Backend public API |
| Backend images | `wisent-mig-images-blue/green`, `wisent-images-regional-mig`, `wisent-bs-images*`, `wisent-images-api-backend`, `wisent-images-cdn`, `wisent-images-vpc/router`, image health/firewall assets | Wisent Backend image service |
| Backend inference | `wisent-mig-inference-blue/green` in three zones, `wisent-bs-inference-*`, `wisent-inference-internal-bs/fwd/hc` | Wisent Backend inference, now replaced by Brama/Stado target routing |
| Training | `wisent-mig-training-blue/green`, `wisent-bs-training-blue/green` | label-model and general historical training |
| ComfyUI | `image-gen-comfyui-mig`, `comfyui-static-ip`, `image-gen-comfyui-health`, ComfyUI gateway/8188 firewall rules | Echo, NeedHer and image workflows |
| Long-lived CPU VMs | `content-platform-vm`, `image-video-router-vm`, `weles-apple-auth-prod` plus disks | Echo/Weles, image-video-router and Apple auth |
| Ephemeral GPU workers and experiments | fourteen Stado workers plus VATT, Z-Image interpolation, NeedHer watermark and Sapiens2 workloads | queued compute and one-off research |
| Shared/default network | `default`, 43 subnetworks, 45 routes and default internal/ICMP/RDP/SSH rules | general VM estate |
| Ancillary legacy access | phone proxy, tinyproxy, VNC emulator, account API, status server, code server and bridge-health firewall rules | old operator, account, proxy and development paths; no live VM proves an active consumer |
| Revision archives | 184 images, 165 instance templates and 38 instance settings | generated deployment history for the families above, not 387 separate services |

Other exact compute counts: 20 terminated instances, 20 persistent disks (4.15 TB), 13 instance groups, 13 managers, 12 backend services, 5 health checks, 3 addresses, 3 forwarding rules, 2 networks, 2 URL maps, one backend bucket, one router, one SSL certificate and one HTTP plus one HTTPS target proxy.

## Enabled APIs

These 86 APIs are enabled. Enablement alone does not establish a live caller. Resource-backed use is documented above; Google Workspace/Ads/Android/Translate APIs may be called by off-GCP tools, while many platform/catalog APIs have no project resource at all.

```text
agentregistry.googleapis.com
aiplatform.googleapis.com
analyticshub.googleapis.com
androidpublisher.googleapis.com
apikeys.googleapis.com
apphub.googleapis.com
appoptimize.googleapis.com
apptopology.googleapis.com
artifactregistry.googleapis.com
autoscaling.googleapis.com
bigquery.googleapis.com
bigqueryconnection.googleapis.com
bigquerydatapolicy.googleapis.com
bigquerydatatransfer.googleapis.com
bigquerymigration.googleapis.com
bigqueryreservation.googleapis.com
bigquerystorage.googleapis.com
billingbudgets.googleapis.com
certificatemanager.googleapis.com
cloudaicompanion.googleapis.com
cloudapiregistry.googleapis.com
cloudapis.googleapis.com
cloudasset.googleapis.com
cloudbilling.googleapis.com
cloudbuild.googleapis.com
cloudfunctions.googleapis.com
cloudquotas.googleapis.com
cloudresourcemanager.googleapis.com
cloudscheduler.googleapis.com
cloudsupport.googleapis.com
cloudtasks.googleapis.com
cloudtrace.googleapis.com
compute.googleapis.com
container.googleapis.com
containerfilesystem.googleapis.com
containerregistry.googleapis.com
dataform.googleapis.com
dataplex.googleapis.com
datastore.googleapis.com
deploymentmanager.googleapis.com
dns.googleapis.com
docs.googleapis.com
drive.googleapis.com
driveactivity.googleapis.com
edgecache.googleapis.com
essentialcontacts.googleapis.com
geminicloudassist.googleapis.com
geminidataanalytics.googleapis.com
generativelanguage.googleapis.com
gkebackup.googleapis.com
gmail.googleapis.com
googleads.googleapis.com
iam.googleapis.com
iamcredentials.googleapis.com
iap.googleapis.com
language.googleapis.com
logging.googleapis.com
modelarmor.googleapis.com
monitoring.googleapis.com
networkconnectivity.googleapis.com
networkmanagement.googleapis.com
networksecurity.googleapis.com
networkservices.googleapis.com
observability.googleapis.com
oslogin.googleapis.com
playdeveloperreporting.googleapis.com
privilegedaccessmanager.googleapis.com
pubsub.googleapis.com
recommender.googleapis.com
redis.googleapis.com
run.googleapis.com
secretmanager.googleapis.com
servicemanagement.googleapis.com
serviceusage.googleapis.com
sheets.googleapis.com
slides.googleapis.com
source.googleapis.com
sql-component.googleapis.com
sqladmin.googleapis.com
storage-api.googleapis.com
storage-component.googleapis.com
storage.googleapis.com
storagetransfer.googleapis.com
telemetry.googleapis.com
translate.googleapis.com
vision.googleapis.com
```

## Resources that historical deploy records name but live inventory no longer contains

- Cloud Run: `stado-coordinator`, `wisent-compute-backend`, `asc-webhook-bridge`, historical model-router family.
- Cloud Scheduler: `wisent-compute-cron`.
- Cloud Functions v2: `wisent-compute-tick` and any body-horror detector function.
- Compute instances for the API/image/inference MIG families; only terminated standalone instances remain in the current asset result.
- Secret `wisent-azure-billing-sp`, named by older setup comments but absent from the live 19-secret list.

Absence here means absent from the 2026-08-15 Cloud Asset search result, not proof that no historical log or object remains.

## Evidence

- Live: `gcloud asset search-all-resources --scope=projects/wisent-480400` on 2026-08-15.
- Live: Cloud Run, Cloud SQL, IAM, Workload Identity, Pub/Sub subscription, API key and bucket-prefix metadata reads on 2026-08-15.
- `docs/gcp-compute-retirement-2026-08-15.json`.
- `docs/incidents/2026-07-27-gcp-billing-outage.md`.
- `stado-rs/data/registry.json` and `deploy/azure/stado.config.json`.
- `backends/wisent-backend/ARCHITECTURE.md` and media client source.
- `oko/CHANGELOG.md` and Oko release workflows.

Blocked detail APIs returned `BILLING_DISABLED`; the inventory does not route around that decision and does not propose relinking billing.
