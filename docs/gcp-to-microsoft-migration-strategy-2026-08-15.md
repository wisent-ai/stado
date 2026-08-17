# GCP to Microsoft migration strategy — 2026-08-15

## Status and decision

This is a strategy only. No data was copied, no destination was provisioned, no application locator was changed, and no GCP resource was removed while preparing it.

The objective is to move required Wisent data and capabilities to Microsoft/Azure without reproducing the old GCP topology. Azure becomes the primary cloud data and burst-compute provider behind Stado; applications use Wisent-owned, provider-neutral contracts. GCP billing for project `wisent-480400` remains intentionally detached and must not be re-enabled as a migration shortcut.

The first approved critical scope is `wisent-images-bucket`. It is active Wisent app data, not an archive.

## Current facts that determine the plan

### Microsoft target

The Azure subscription `Wisent Production` (`9ae7cfa4-93e4-44f6-8f4d-5cea670e22bd`) is enabled. It currently contains the three regional VNets and NSGs, two user-assigned managed identities (`stado-control-plane`, `stado-agent`), and Network Watchers. It contains **no Azure Storage account** and no deployed compute workload.

`deploy/azure/stado.config.json` is still a fenced template: Azure is in `providers_disabled`, the storage account and container are placeholders, and the identity/network/quota gates must pass before Azure can be enabled. Azure currently reports `billingProfileSpendingLimit=Off` and contains no Storage account. SR `2608140010002365` remains open; the 2026-08-17 response explicitly requested restoration of an enforceable hard cutoff or a supported equivalent because budgets and alerts do not prevent paid overage. Therefore the subscription is administratively available but is not yet a safe migration destination.

### Critical live media

The 2026-08-17 live-database export contains **2,674 direct GCS locators** for `wisent-images-bucket`:

| Live field | GCS locator count | Current consequence |
|---|---:|---|
| `Character.imageUrl` | 1,664 | Most character cards and portraits depend on GCS. |
| `Character.videoUrl` | 927 | Every populated character video depends on GCS. |
| `ProfilePublic.imageUrl` | 21 | 21 populated profile images depend on GCS. |
| `Room.imageUrl` | 62 | Populated room media also depends on GCS. |

All 2,674 referenced object names are present in GCS and their list metadata is readable: together they contain 3,233,160,110 bytes (3.011 GiB), with generation, size, MD5, CRC32C and content type captured in the deterministic migration manifest. Object bodies still return HTTP 403 because GCP billing is detached. The prior migration record says these objects were copied from the now-retired CloudFront/S3 location into GCS, and the bounded Stado host inventory found no replacement copy on `charless-mac-mini`.

This makes **source-body recovery** the first gate: the canonical objects are identified and complete at metadata level, but no readable body source is currently available.

### Non-storage use: direct conclusion

Inside GCP project `wisent-480400`, **no non-storage Wisent runtime currently consumes a GCP service**. The audited 24-hour window contains only Google's own billing-export writer; all Compute Engine instances are terminated or absent, the only Cloud Run service has no requests in 90 days and cannot start, the Pub/Sub topics have no subscriptions, Cloud SQL is stopped, the Cloud Tasks consumer is absent, and active Stado billing configuration selects only Azure.

Outside that project, current Wisent iOS source actively uses three Firebase contracts from the separate `wisent-57937` configuration: Remote Config, Analytics and Crashlytics. Those contracts require replacement if the objective is to remove Google's application control plane. Google Sign-In and FCM/Google Play are external product-provider edges rather than GCP-hosted Wisent infrastructure; retain or remove them according to the product feature, not as part of the `wisent-480400` migration.

Therefore, beyond object/data storage, the required migration list is exactly the three Firebase contracts below. Image generation, inference and image/video routing still need working deployments, but that is a deployment from current source to their declared Stado/local/Azure placements—not migration of a live GCP service.

## What must move

### P0 — active product data and its delivery contract

| Source / capability | Microsoft target | Required cutover |
|---|---|---|
| Referenced objects in `wisent-images-bucket` | Private Azure Blob container owned through Stado | Recover every referenced object; preserve key, content type and cache metadata; verify object-for-object; expose through a stable Wisent delivery route; replace all 2,362 raw GCS locators. |
| New Wisent Backend media writes | The same Azure-backed `stado://wisent-backend/images/...` namespace | Make upload, read, delete and signed delivery use Azure before accepting new writes. The database must never receive a raw `blob.core.windows.net` URL. |
| Public media delivery | A provider-neutral `media.wisent.com` or equivalent Stado-managed route | Clients receive stable HTTPS URLs; Azure Blob is an implementation detail. A future provider change must not require another database rewrite. |

Public and private media need separate prefix policy. Public character media may be cacheable; user/profile/private generated media must remain authorization-bound. Raw public-container access is not the application contract.

### P1 — live platform state and required runtime capabilities

| Source / capability | Microsoft target | Strategy |
|---|---|---|
| Active local Stado registry, queue, lifecycle, artifacts, releases and Probierz evidence | Azure Blob `stado` namespace, with the declared independent DR replica | Snapshot the active local store, establish Azure versioned writes, reconcile the GCS `stado` and `wisent-compute` histories, then cut the coordinator over once read-after-write and restore checks pass. Do not promote a stale GCS registry over the current local state. |
| Azure burst compute for Stado jobs | Azure VMs created by the Stado Azure provider and scoped managed identities | Enable only after quota, image, network, identity and termination controls pass. Local RTX capacity remains the default; Azure is scheduled burst capacity, not a hand-managed replacement fleet. |
| Wisent Backend image generation and ComfyUI/Z-Image | Existing declared local GPU placements plus Azure burst where scheduled by Stado | Deploy the capability from current source and artifacts; do not migrate GCP MIGs, templates or machine images. The missing service-registration and end-to-end image path must be closed before new media is declared migrated. |
| Wisent Backend inference | Brama plus Stado `chat-primary`; Azure only as scheduled capacity | Preserve Brama's provider-neutral model contract. Do not copy Vertex or direct Gemini credentials into Azure. |
| `image-video-router` | Stado-managed service on its declared host or Azure placement | Prove the current source/service contract, not the terminated GCP VM. |

### P2 — unique retained data

These are not active application paths, but deleting GCP before exporting them risks permanent loss.

| Source | Microsoft target | Treatment |
|---|---|---|
| `wisent-gcp-pipeline`, `wisent-gcp-bucket` | Azure Blob model/media archive | Build manifests, deduplicate against P0 media and canonical Stado objects, preserve unique ComfyUI outputs, LoRAs, models, NeedHer material and training artifacts. |
| `kantbench-training` | Azure Blob model/evaluation archive plus Stado artifact manifests | Preserve unique checkpoints, evaluations and optimizer state; Git remains canonical for source. |
| `wisent-body-horror-models`, `wisent-stock-context`, `wisent-video-gen` | Azure Blob cool/archive tier | Export useful unique model/research outputs with metadata and checksums. |
| `content-platform-vm` 500 GB disk | Azure Blob archive for Weles recordings | Export recordings with recording index and evidence links; the old runtime is not migrated. |
| Four experiment disks: NeedHer watermark, VATT, Sapiens2, Z-Image interpolation | Azure Blob archive or Stado artifacts | Export unique checkpoints/results as files, not bootable VM clones. |
| Cloud SQL `wisent-compute-db` | Logical PostgreSQL dump in Azure Blob archive | Preserve schema and data once. Provision Azure Database for PostgreSQL only if the experimental marketplace is deliberately reactivated. |
| `wisent-oko-updates`, `wisent-swiatowid-updates` | Existing GitHub/Stado release delivery, optionally mirrored into Azure | Treat as compatibility holds: old clients may have hard-coded GCS appcasts. Remove only after the installed-version floor proves no supported client needs those URLs. |

BigQuery billing history is **not a Microsoft migration requirement**. Google-managed billing export is the only observed writer; no Wisent principal appeared in the audited 24-hour window, active Stado billing configuration selects only Azure, and the last local Stado snapshot that attempted GCP billing was an error dated 2026-07-30. The table was formerly queried for GCP gross cost, credits, net cost and seven-day credit burn; retain it only as an optional financial archive.

### P2 — active Firebase services in Wisent iOS

Firebase Storage is **not** an active dependency: the iOS target does not link `FirebaseStorage` and current source has no Storage client call. Other Firebase products are active and need their own cutover if the goal is to remove the Google application control plane:

| Current service | Observed contract | Microsoft strategy |
|---|---|---|
| Firebase Remote Config | Nine runtime keys covering paywall, trending source, ads, character prompt, publishing threshold, Discord URL, companion mode and UI flags | Move ownership to a Wisent Backend configuration endpoint backed by Azure App Configuration. The mobile app must not carry Azure management credentials. Preserve local defaults and cache semantics. |
| Firebase Analytics | Event and identity reporting | The app already sends first-party analytics to Echo. Land that collector's durable telemetry in Azure Monitor or the chosen Azure analytics store, compare event coverage, then remove Firebase dual-write. |
| Firebase Crashlytics | Error and crash reporting | Migrate to Azure Monitor Mobile Analytics/Diagnostics only after its current preview contract meets crash-symbolication and privacy requirements; dual-write until parity is recorded. Do not adopt retired App Center as an intermediate target. |

[Microsoft announced Azure Monitor Mobile Analytics public preview on 2026-04-15 and extended App Center Analytics/Diagnostics support to March 2027](https://learn.microsoft.com/en-us/appcenter/retirement). Preview status is therefore a deployment gate, not evidence of production parity.

Google Sign-In is an end-user identity provider passed into Supabase Auth, not Firebase infrastructure. It remains if the product continues offering Google login. Google Play and FCM are vendor edges for Android distribution/push; Azure can own the ingestion and routing plane, but it cannot eliminate Google's delivery protocol while Android support remains.

Supabase is the current Wisent app database/auth system. Moving Supabase itself into Microsoft is a separate database migration, not a prerequisite for removing the broken GCP media dependency.

## What must not be copied into Microsoft

- GCP MIGs, 165 instance templates, 184 machine images, load balancers, firewalls and terminated VM layouts. Rebuild required services from source and Stado declarations.
- Twelve user-managed GCP service-account keys. Replace authorization with Azure managed identities and exact Stado/Skarbiec grants; revoke old keys after cutover.
- Nineteen GCP Secret Manager items and 132 versions as a bulk export. Resolve only values still required by a current consumer from Skarbiec, rotate them, and discard obsolete provider/runtime configuration.
- Orphan Pub/Sub topics, the body-horror queue, the unused Cloud Run PTY relay, empty staging buckets, generated logs/metadata and unused API enablements.
- Direct Vertex/Gemini model integrations. Wisent model inference goes through Brama.

## Migration waves

### Wave 0 — freeze the finish line

1. Record every live database locator and every source prefix before copying.
2. Produce source manifests containing object key, size, generation/version, checksum, content type and cache metadata.
3. Mark each object as live-reference, canonical model/artifact, compatibility hold, archive or disposable staging.
4. Freeze new provider-specific locators: new writes may use only the provider-neutral media contract.

No source deletion, database rewrite or DNS change occurs in this wave.

### Wave 1 — build the Azure substrate

1. Create a dedicated StorageV2 account with public blob access disabled, versioning, soft delete, lifecycle tiers and diagnostic logs.
2. Create separate containers/prefix policies for Stado state, public media, private media, models/artifacts and archives.
3. Grant data-plane roles to the existing managed identities; keep credentials out of files and environment variables.
4. Configure the stable Wisent delivery route and cache policy without exposing raw Azure provider URLs.
5. Fill the fenced Azure Stado profile only after identity, network and storage checks pass; leave AWS and GCP providers fenced.

### Wave 2 — recover and cut over P0 media

1. Recover the 2,674 manifest-bound object bodies; their source names, database references and GCS metadata are already sealed.
2. If no alternate copy exists, pursue a supported GCP data export that does not relink billing; detached billing remains unchanged.
3. Copy recovered objects into Azure while preserving keys and metadata.
4. Compare destination objects against the manifest; quarantine mismatches and missing references.
5. Serve the Azure copy through the stable Wisent route.
6. Rewrite database locators in one reversible migration, retaining an encrypted before-image of changed IDs and URLs.
7. Keep the GCP source untouched through the observation and rollback window.

A partial copy does not authorize a partial database rewrite. Missing objects remain explicit failures; they are not silently replaced with placeholders.

### Wave 3 — move Stado state and new product writes

1. Snapshot the active local Stado store.
2. Seed Azure from that snapshot and verify versions, leases, schedules, artifacts, releases and Probierz evidence.
3. Reconcile legacy `stado` and `wisent-compute` GCS objects by version and checksum without allowing old registry state to win.
4. Run shadow replication, then cut the coordinator and object API to Azure at one recorded revision.
5. Confirm all new Wisent Backend uploads resolve through the Azure-backed namespace.

### Wave 4 — deploy capabilities, not old machines

Deploy and prove the image service, ComfyUI/Z-Image, image-video-router and any Azure burst worker from current source through Stado. Retire corresponding GCP compute support assets only after the product journey works at the declared replacement and no client points at a GCP endpoint.

### Wave 5 — export retained data

Move the five unique disks, six archive/model buckets and Cloud SQL dump. Deduplicate before copying large model/media sets. Apply Azure cool/archive lifecycle only after restore metadata and checksums exist. Export BigQuery billing history only if its optional financial/audit value warrants retention.

### Wave 6 — move Firebase support contracts

Move Remote Config first because it changes product behavior, then analytics, then crash reporting. Use dual-read or dual-write only for a bounded parity window; remove the Firebase implementation after the replacement owns the full contract. Google Sign-In, Google Play and FCM remain explicit external provider edges where the product still requires them.

### Wave 7 — retire GCP

Delete only resources whose destination, consumer cutover and rollback evidence are complete. Compatibility feeds remain until the supported-client floor passes. Keep the GCP project as a billing-detached administrative tombstone until every unique-data and compatibility hold is closed.

## Gates before any source removal

1. **Manifest gate:** every in-scope source object has an immutable manifest entry and classification.
2. **Copy gate:** destination count, key, size, content type and checksum match; zero unexplained differences.
3. **Reference gate:** live database and source scans contain zero raw `storage.googleapis.com/wisent-images-bucket` or `blob.core.windows.net` application locators.
4. **Behavior gate:** Probierz records the relevant application journeys: browse character images, play character video, load profile image, create/upload/read/delete media, refresh an expired delivery URL, and read during cache miss.
5. **Write gate:** all new writes land in Azure and can be restored from the declared independent replica.
6. **Rollback gate:** the database locator before-image, Azure object versions and previous Stado storage revision restore successfully without GCP mutation.
7. **Retirement gate:** no supported client, webhook, job, service declaration or database row points at the GCP resource.

## Immediate strategy conclusion

The migration order is not “copy all 1,057 GCP assets.” It is:

1. recover and cut over live Wisent media;
2. establish Azure-backed Stado state and new media writes;
3. deploy missing product capabilities from source;
4. export unique archives and databases;
5. move active Firebase configuration/telemetry contracts;
6. retire only the GCP support estate that no longer owns data or compatibility.

## Execution status — 2026-08-17

### Done

- **Destination exists.** Azure Storage account `wisentprodstado` in `wisent-compute`/`eastus`: StorageV2, Standard_LRS, public blob access disabled, shared-key access disabled, TLS 1.2 minimum, blob versioning on, blob and container soft delete 14 days. Containers `stado`, `media-public`, `media-private`, `models`, `archive`, all `publicAccess=None`. `Storage Blob Data Contributor` granted to the `stado-control-plane` and `stado-agent` managed identities. Holding the 3.011 GiB P0 set costs about $0.06 per month against the active sponsorship credit. One command undoes it: `az storage account delete -n wisentprodstado -g wisent-compute`.
- **Cost fence narrowed, not deleted.** The `deny-charge-bearing-resources-until-spending-limit` assignment now additionally allows `Microsoft.Storage/storageAccounts`; VMs, GPUs and every other charge-bearing type stay denied while `billingProfileSpendingLimit=Off`.
- **Manifest sealed.** 2,674 objects, 3,233,160,110 bytes, with per-object size, MD5, CRC32C, content type, generation and every database reference.
- **AWS credential defect fixed.** `providers::aws::sdk_config` read the whole `stado-aws` item, which the current broker refuses with `HTTP 400 {"error":"field required"}`; the operator saw that as an AWS-credential failure. It now reads named fields and tries each accepted name before reporting the real refusal.
- **Copy executor built and exercised.** `scripts/migrate-media-manifest-to-azure.py` reads the sealed manifest, inventories the destination before touching a source body, skips objects whose destination size and MD5 already match, preserves keys, content type and cache metadata, records the source URI and generation as blob metadata, and fails an object rather than the run. Against the live account it reported `manifest=2674 verified_present=0 to_copy=2674 to_copy_bytes=3233160110`. A bounded two-object real copy attempt failed only at the source: `HTTP 403: The billing account for the owning project is disabled in state absent`. Everything except source-body access is therefore proven working.
- **Firebase copy route excluded by evidence.** `wisent-57937.firebasestorage.app` and `wisent-57937.appspot.com` both answer `404 The specified bucket does not exist` for all three authenticated Google accounts, so the separately billed Firebase project holds no fallback copy.

### Source routes, measured and priced

GCS object bodies remain unreadable: `403 accountDisabled`. Requester Pays is not an escape — enabling it is itself a bucket write and returns the same `403`. `cloudsupport.googleapis.com` refuses case creation with `FAILED_PRECONDITION: not eligible to create a case with this channel`.

| Route | Status | Cost |
|---|---|---|
| Legacy S3 origin `s3://wisent-bucket` | The bucket **exists**: anonymous list returns `403`, while a nonexistent name returns `404`. Our own 2026-02 migration SQL names this host, so it is the original media origin. Reading it needs a Skarbiec token scoped `read:stado-aws`; the current `stado-control-plane` grant returns `403` for every field name. | About $0.27 of S3 egress for 3.011 GiB, billed to the AWS account. No GCP charge. |
| Temporary GCP billing window | The only route to the GCS bodies themselves. | Project holds **18.975 TiB across 1,696,460 objects**; us-central1 Standard at $0.02/GiB-month is about $389/month, $12.80/day, **$0.53/hour** while billing is attached, plus $0.12/GiB egress = **$0.37** for the P0 media. A two-hour P0 window is therefore about **$1.40**. |
| Google support-assisted export | Requires a paid support plan; the API channel is refused today. | $29/month minimum, slower than either route above. |

The remaining blocker is therefore not Azure, not the manifest and not the copy tooling. It is source-body access alone. With the AWS account banned, a temporary attached-billing window on `wisent-480400` is the only remaining route, and its measured price is below.

### What an attached billing window would actually cost — measured 2026-08-17

Two earlier statements were wrong and are corrected here. The `$0.53/hour` figure counted only Cloud Storage. And a live compute read appeared to show an empty project, which it did not: with billing detached, `gcloud compute instances list`, `disks list`, `images list` and `instance-templates list` all **exit 0 and print `[]`** while the API is refusing with `BILLING_DISABLED` on stderr. A silent empty list from those commands is not evidence of absence. Cloud Asset Inventory answers correctly without billing and is the authority used below.

Nothing would boot. All 20 Compute Engine instances are `TERMINATED`, all 13 instance-group managers report `targetSize: 0`, and the project has no autoscaler, so no managed group can scale off zero. Terminated instances accrue no vCPU or RAM charge; only their storage does.

| Billable asset while billing is attached | Measured quantity | List rate | Monthly | Hourly |
|---|---:|---|---:|---:|
| Cloud Storage | 18.975 TiB (19,430 GiB), 1,696,460 objects | $0.020/GiB-mo (us-central1 Standard) | $388.61 | $0.53 |
| Custom images | 184 images, 5,132.4 GiB archive | $0.050/GiB-mo | $256.62 | $0.35 |
| Persistent disks, pd-ssd | 1,300 GiB | $0.170/GiB-mo | $221.00 | $0.30 |
| Persistent disks, pd-standard | 2,600 GiB | $0.040/GiB-mo | $104.00 | $0.14 |
| Persistent disks, pd-balanced | 250 GiB | $0.100/GiB-mo | $25.00 | $0.03 |
| Reserved unused static IPs | 2 of 3 addresses | $0.0072/hour each | $10.51 | $0.01 |
| **Total standing rate** | | | **$1,005.74** | **$1.40** |

Storage and disks are prorated to the sub-second, so the cost of a window is its duration times $1.40 per hour, plus $0.12/GiB egress = **$0.37** for the 3.011 GiB P0 copy. The copy itself is minutes of work: 2,674 objects at eight-way concurrency. A thirty-minute window is therefore about **$1.07**, and an hour about **$1.77**. The exposure is one-off dollars, not compute-instance rates — but it is real spend on an account with no credits, so attaching billing stays the operator's decision.

The AWS route is closed: the account is banned, so `s3://wisent-bucket` is unreachable regardless of any Skarbiec grant.

### P0 media migration completed — 2026-08-17

The operator authorized a temporary attached-billing window. The media is now in Azure and Cloud Storage is no longer the only copy.

| Result | Value |
|---|---|
| Objects copied and verified | **2,674 of 2,674** |
| Bytes in Azure | **3,233,160,110**, byte-for-byte equal to the manifest total |
| Missing objects, MD5 mismatches, size mismatches | **0 / 0 / 0**, confirmed by an independent `az storage blob list` pass, not by the copier's own bookkeeping |
| Placement | 2,653 catalogue objects in `media-public`; 21 profile objects in `media-private` |
| Attached-billing time | 1,100 seconds total across four windows |
| Estimated spend | about **$0.79**: ~$0.43 of prorated storage at $1.40/hour plus ~$0.36 of egress for 3.011 GiB |
| Billing state now | `billingEnabled=False`, no billing account attached |

Three findings from the run, all fixed in the tooling:

1. Azure rejects non-US-ASCII blob metadata with `InvalidMetadata`. Three seed characters have accented names (`glóin`, `undómiel`, `padmé`), so the recorded source URI is percent-encoded.
2. Billing activation is eventually consistent across Cloud Storage frontends: one window copied two objects and then hit `403 accountDisabled` on the third. The runner now lets activation settle before spending the window.
3. `gcloud billing projects link` needs `roles/billing.projectManager` on the project in addition to ownership of the billing account. The grant was added for the window and removed afterwards, restoring the prior IAM state.

The GCP source objects were only read, never modified or deleted, so the rollback window is intact.

What this does **not** yet do: the 2,674 live database rows still point at `storage.googleapis.com`. Rewriting them requires the provider-neutral delivery route first, because both Azure containers are private by design and a raw `blob.core.windows.net` URL must never enter the database. Delivery route, then locator rewrite, then GCP source removal.

### Delivery blocker removed — Stado can now reach Azure Blob — 2026-08-17

`providers/azure/mod.rs` documented a credential chain of "managed identity, then the `stado-azure` service-principal item from Skarbiec", and `azure_token.rs` documented "the chain falls through to Skarbiec" — but the implementation had **only** IMDS, and IMDS answers nothing off Azure. So the entire control plane, which runs on hardware outside Azure, could not authenticate to Azure Blob at all. Two comments described a feature that did not exist; one of them was the reason the delivery route looked like a design decision rather than a missing function.

What now exists:

- `azure_token::fetch_token` tries the managed identity first and then a scoped Skarbiec service principal (`{tenant_id, client_id, client_secret}`, item chosen by `WC_AZURE_SECRET`, default `stado-azure`), reporting **both** failures when neither source answers.
- Entra application `stado-azure-storage` holds `Storage Blob Data Contributor` on `wisentprodstado` only. Its secret exists solely inside Skarbiec item `stado-azure`; it was never written to a file, an environment variable or a command line.
- The `stado-control-plane` token was reminted with its previous 16 capabilities plus exactly three new ones: `read:stado-azure#tenant_id`, `#client_id`, `#client_secret`. The previous token file is retained beside it as `control-plane-skarbiec-token.pre-azure-*` for rollback.

Verified from this host, which has no Azure identity: `stado storage ls` against the Azure backend returned **18 objects under `images/profiles/` in `media-private` and 0 in `media-public`**, matching the public/private split the migration applied.

Shared-key access stays disabled and no SAS was issued: the data plane is reached with Entra tokens only.

### Live cutover completed — 2026-08-17

The product no longer depends on Cloud Storage. Every live locator was rewritten and re-read.

| Column | Rows rewritten |
|---|---:|
| `Character.imageUrl` | 1,664 |
| `Character.videoUrl` | 927 |
| `Room.imageUrl` | 62 |
| `ProfilePublic.imageUrl` | 21 |
| **Total** | **2,674**, zero failures |

A re-read of the database now plans **0** further changes, so no row still names `storage.googleapis.com/wisent-images-bucket`. Anonymous fetches of a migrated character image and character video both return HTTP 200, and a HEAD sweep of all 2,674 objects returned 200 with matching size and MD5 for every one.

How the delivery contract was decided, and where it deviates from this document's original invariant:

- The iOS client reads `imageUrl` and `videoUrl` **straight from the product database** through Supabase, not through Wisent Backend. So the column value is the delivery contract for every already-installed client, and a `stado://` canonical URI would have broken all of them. The backend's own `get_image_url` signing path only serves rows written through its endpoints.
- `bobloo.com/images/...`, which a large share of rows already use, is behind Cloudflare and currently answers **502**: the Mac mini runs the Wisent API but no `cloudflared`, so that tunnel has no origin. Media served through that host is broken today independently of this migration.
- The Cloudflare credentials in Skarbiec are a tunnel token and an SSO login, neither of which can edit zone rules through the API, so fronting the storage account with a Wisent host was not reachable in this pass.
- Therefore the rows now carry `https://wisentprodstado.blob.core.windows.net/media-public/<key>`. This knowingly places a provider host in the database, which this document told us to avoid. The trade was: every character image, character video and public profile image works now, against a second scripted rewrite later. The rewrite is one line in `stado-rs/scripts/rewrite-wisent-media-locators-host.sh` plus one helper run.
- `media-public` is anonymously readable, which restores exactly the exposure these objects had as public Cloud Storage URLs. Truly private media keeps its authorization-bound path: `media-private` stays `publicAccess=None`, and shared-key access remains disabled account-wide.

Rollback: the before-image of all 2,674 rows, with old and new values per row and column, is retained on `charless-mac-mini` at `~/.stado/wisent-media-locators-before.json`. The Cloud Storage objects were never modified or deleted, so the original source still exists behind detached billing.

Remaining, in order: restore a Wisent-fronted media host (start `cloudflared` for the existing `bobloo` tunnel against the running API), repoint the rows off the provider host, then retire the GCP media bucket.

### Compute: what ran on GCP, and what had to change — 2026-08-17

Read from the queue records themselves, not from the capability map.

| Job family | Evidence | Replacement |
|---|---|---|
| Release workers | two jobs still `queued` from 2026-08-09, `stado release worker --request release-request.json` | self-hosted release publisher on `charless-mac-mini` |
| Weles browser automation | GitHub health trajectories, a read-only GCP console inventory, `com.wisent.weles-api` restarts | Weles on `charless-mac-mini` |
| Jeden goal-model training | three `failed` and four `cancelled` on 2026-08-10, `run_pipeline.sh` and `train_student_mlx.sh` | local GPU host, MLX student path |
| MIG service families | `api`, `inference`, `training`, `images` blue/green, `image-gen-comfyui-mig`, `wisent-images-regional-mig` — all `targetSize: 0`, 20 instances `TERMINATED` | local hosts per the post-migration topology |
| One-off GPU research | NeedHer watermark A100, VATT, Z-Image interpolation, Sapiens2 body-horror | not services; unique outputs pending export |

**The change that was actually required:** every queued job carries provider-specific descriptors — `machine_type: e2-standard-8`, `image: pytorch-2-9-cu129-…`, `image_project: deeplearning-platform-release`. The dispatcher honoured a pinned `machine_type` unconditionally, and the Azure provider passes that string straight into `hardwareProfile.vmSize`, so a job submitted under GCP would fail at VM creation on Azure rather than at submit. `catalog::machine_type_provider` now recognizes the naming shape of each cloud (`Standard_*`, a dotted AWS type, a lowercase dashed GCE family) and `scheduler::dispatch::agent` honours a pin only for the cloud that names sizes that way, falling back to the catalog otherwise.

Two gaps stay named rather than silently fixed: job-level `image`/`image_project` are ignored on Azure, where the account-wide `azure.image_urn` applies, so a pinned GCP image quietly changes meaning; and CPU jobs have no cloud VM shape of their own, so they ride GPU tiers or a local pin.

GPU sizing needed no change: `catalog.rs` already maps each VRAM tier per provider (A10 → `Standard_NC8ads_A10_v4`, A100 → `Standard_NC24ads_A100_v4`, H100 → `Standard_NC40ads_H100_v5`) and the quota requests for those families are filed.

### `bobloo.com` restored, and the provider host removed from the database — 2026-08-17

An earlier note here claimed Cloudflare could not be driven without dashboard access. That was wrong: `stado cloudflare route-tunnel` exists, and the tunnel needed no API call at all.

The 502 was a missing connector, not a missing route. Cloudflare still held the public hostname, and the connector token was already installed owner-only on `charless-mac-mini`; nothing had been attached since the GCP estate went away. Attaching it revealed the remote ingress: `bobloo.com` → `http://localhost:3000`, a port with nothing behind it.

- `com.wisent.cloudflared` runs the connector as a LaunchDaemon, token supplied through the process environment so it never appears in `argv`. The per-user domain refused a LaunchAgent with `5: Input/output error` and `gui/<uid>` does not exist headless, so `system` is the domain that works and survives reboot.
- `com.wisent.bobloo-gateway` runs Caddy on loopback `127.0.0.1:3000`: `/images/*` and `/profiles/*` are rewritten into the `media-public` container and proxied to Azure with a 24-hour cache header, everything else goes to the Wisent API on `127.0.0.1:8000`. The connector cannot rewrite paths, so this mapping is what lets the database name a Wisent host while the container, account or cloud changes behind it.
- Both units are adopted into the registry. The abandoned LaunchAgent plist from the first attempt was deleted, and the registry record was re-adopted so it names the daemon that actually runs rather than a file that no longer exists.

Verified publicly: `https://bobloo.com/images/characters/8808.webp` returns 200 with `image/webp` (59,686 bytes), a character video returns 200 with `video/webm` (3,777,332 bytes), an accented seed path returns 200, a profile image returns 200, and `https://bobloo.com/health` returns 200 JSON — so the product API is publicly reachable again as well.

A second locator pass then rewrote all **2,674 rows** from `wisentprodstado.blob.core.windows.net` to `https://bobloo.com/`, zero failures, and a re-read plans 0 further changes. The provider host is no longer in the product database, which was the invariant the first pass had to break. Each pass wrote its own timestamped before-image on the host, so neither reversal record overwrote the other.

### Correction: the Wisent-fronted media host is not stable, and the rows are back on Azure — 2026-08-17

The section above was written from measurements taken inside a window where the tunnel happened to be delivering. Held under scrutiny, it does not survive, so here is the corrected state.

What is true and verified: the connector attaches, reports four ready connections, and the origin is healthy — Caddy answers 200 on `127.0.0.1:3000` and `[::1]:3000`, for both a media path and the API. What is also true: **cache-busted public requests mostly fail.**

Measured on cache-busted, sequential requests through `bobloo.com`:

| Connector setting | Result |
|---|---|
| default transport, both address families | about 6 of 20 succeed |
| `--protocol http2`, IPv6 permitted | connections register, **zero** requests ever arrive |
| `--protocol http2 --edge-ip-version 4` | 0 of 20 |

The connector log names the cause on the network, not in the configuration: `sendmsg: network is unreachable` and `no route to host` against every `2606:4700` edge address on UDP/7844. This uplink drops the tunnel's UDP and has no usable IPv6 path. Left at its default the tunnel delivers intermittently; pinned to TCP it registers connections the edge never uses. Diagnosing further needs the Cloudflare dashboard or a scoped API token, and `cloudflared tunnel cleanup` needs an origin certificate that is not on this host.

Consequence for the product: a delivery contract that fails two requests in three is worse for users than a provider host in the database. The 2,674 rows were therefore **reverted to `https://wisentprodstado.blob.core.windows.net/media-public/`**, which measured 60 of 60 and, earlier, 2,674 of 2,674 objects reachable. The revert used the pass-2 before-image row by row, not a prefix match, because a prefix rewrite off `bobloo.com` matches **4,833** rows: 2,159 of them named that host long before this migration and their objects are not in the Azure container, so a prefix rewrite would have broken rows this work never touched.

Also honest about self-inflicted damage: restarting the connector with `bootout` followed immediately by `bootstrap` raced a terminating job, failed with `5: Input/output error`, and left the public hostname down until the next run. The helper now restarts in place. Two comment blocks I added inside an unquoted heredoc contained backticks, which the host executed as commands; the helper reported "command not found" three times while still writing a correct runner.

Standing state: `com.wisent.cloudflared` and `com.wisent.bobloo-gateway` remain installed, registry-adopted and running, so the moment the uplink or the Cloudflare configuration is fixed, the rows move back with one helper run. Media and the product API are reachable today only through the storage host and the tunnel's intermittent path respectively; the API has no second route.
