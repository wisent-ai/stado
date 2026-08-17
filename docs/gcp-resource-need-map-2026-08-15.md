# GCP resource necessity map — 2026-08-15

## Approval status

**The user has approved one migration requirement:** `wisent-images-bucket` is
critical Wisent app data and must be migrated to Microsoft/Azure. The remaining
classifications below are an assistant-authored assessment based on repository
and live-inventory evidence, not an operator-approved retention or deletion
plan. Labels such as `DELETE` describe the assessment result and do not
authorize a destructive operation.

## Assessment conclusion

**The evidence indicates that no Wisent capability needs to remain permanently in GCP.** The assessment's proposed terminal state for project `wisent-480400` is a retained administrative tombstone with billing detached and no Wisent workload, credential, public endpoint or unique data left inside it.

The assessment recommends preserving two things before that state is safe:

1. **Legacy desktop update compatibility:** the two public Sparkle feeds, until old Oko/Swiatowid installations have a proven path to the GitHub/Stado feed.
2. **Unique or unclassified data:** five persistent disks, selected buckets, historical billing data and the stopped Cloud SQL database, until their useful contents are exported or deliberately discarded after inspection.

The assessment classifies everything else as already replaced, an inert deployment artifact, a credential for a retired path, metadata generated around another resource, or an API/quota declaration that no workload needs.

## Meaning of the proposed classifications

| Proposed classification | Meaning |
|---|---|
| `HOLD-COMPAT` | No new system should use it, but old installed clients may still address it. |
| `MIGRATE-THEN-CUTOVER` | Active product data must be copied to Azure object storage behind Stado, verified object-for-object, and cut over at every live locator before the GCP source can be removed. |
| `OPTIONAL-ARCHIVE` | No current product or control-plane capability needs the data; export only when its historical/audit value justifies retaining it. |
| `EXPORT-THEN-DELETE` | The data may matter; the GCP runtime/resource does not. Export, verify the destination and remove the source. |
| `DEDUP-THEN-DELETE` | Likely replicated or historical data; compare with canonical storage, preserve only unique objects and remove the GCP copy. |
| `DELETE` | No current capability or unique-data requirement justifies retention. |
| `CASCADE` | Google-generated metadata/support object; let deletion of its parent remove it or remove it after the parent. |

`DELETE` is a necessity assessment, not user approval and not evidence that the current detached-billing control plane will accept the deletion command.

## Capability-level map

| Capability | Evidence of Wisent need | Evidence of GCP need | Proposed classification |
|---|---|---|---|
| Stado coordinator, queue, object and release APIs | Yes | No | Already placed on `charless-mac-mini` with Azure Blob primary storage and S3 DR; GCP copies are migration/history data only. |
| Brama model routing | Yes | No | Brama owns model access; old GCP identities, Vertex configuration and direct Gemini keys are not a valid route. |
| Wisent Backend public API | Yes | No | Replaced on `charless-mac-mini`; all API MIG/LB/template/image assets are deletable. |
| Echo and Weles durable worker duties | Yes | No | Replaced on `charless-mac-mini`; preserve only unexported recordings from the old disk. |
| Weles Apple authentication | Yes | No | Replaced on `charless-mac-mini`; old VM and disk are deletable. |
| Oko internet PTY relay | Yes | No | Current Oko code defines generic publisher, viewer and control clients, but no current source/configuration binds them to the GCP endpoint. This Mac's publisher URL is empty; a viewer can instead receive a dynamic relay URL from an active `oko_live_sessions` row. Cloud Run recorded no requests and cannot start because detached billing prevents its Secret Manager read, so this GCP deployment is classified `DELETE`. |
| Wisent Backend inference | Yes | No | Target is Brama plus Stado `chat-primary`; keeping inert GCP MIG assets does not restore the missing/offline target. |
| Wisent Backend direct image generation | Yes | No | Target is local GPU 1; keeping old GCP image assets does not close the current service-registration gap. |
| ComfyUI / raw image workflows | Yes | No | Target is local GPU 2 and `image-video-router`; old GCP MIG/gateway assets are not needed. |
| Image/video routing | Yes | No | Target is `image-video-router` on the RTX host; old terminated VM is not needed. |
| Training and batch GPU capacity | Yes | No | Local GPU 3 plus Stado-scheduled Azure burst; historical GCP workers, quotas and templates are not needed. |
| Body-horror detector | No current product contract found | No | Queue has no executing service. Preserve model/data once if useful; retire queue, identity and GCP runtime surface. |
| RevenueCat / Play Store notification ingestion | Potential product capability, but current GCP path is incomplete | No | Topic has no subscription. If the integration remains desired, it needs a supported consumer outside GCP; the orphan topic and service account do not provide it. |
| App Store Connect webhook bridge | Potential release capability, but no live GCP service | No | Current release workflows use GitHub/Stado. Retire old bridge identity and secrets after confirming no external webhook still targets the dead endpoint. |
| Historical billing analytics | Historical data only | No runtime | Export the BigQuery table; no new GCP billing events will arrive while billing remains detached. |
| Old stock, video and research experiments | Archive value only | No | Export useful results/models and retire their GCP containers. |

## Unused GCP PTY relay stack

The GCP relay is a deployed but nonfunctional stack: the Cloud Run control plane reports the revision as `Ready`, while runtime logs show repeated startup aborts on the Secret Manager read and request logs contain no requests in the last 90 days.

| Resource | Necessity |
|---|---|
| Cloud Run `swiatowid-pty-relay` | `DELETE`; no observed requests and instances abort startup |
| All seven `swiatowid-pty-relay` revisions | `DELETE`/`CASCADE` |
| All relay image digests in `cloud-run-source-deploy` | `DELETE` |
| `gs://run-sources-wisent-480400-europe-west1` relay source object | `DELETE`; canonical source exists in Oko |
| Secret `swiatowid-pty-relay-token` | `DELETE`; a future Stado relay uses exact Skarbiec service/client grants |
| Default Compute service account dependency | `DELETE`; the failed Cloud Run service is its only identified current binding |
| Run, Artifact Registry, Secret Manager, IAM, logging and monitoring APIs | Not justified by this relay; retain only where data retirement still needs them |

The product capability remains explicit in Oko: one long-lived Stado-managed `oko-pty-relay`, TLS, exact Skarbiec grants and one instance because session pairing is in process memory. That is a future/current product deployment concern, not a dependency on this unused GCP service.

### Source consumer trace

The current code has consumers for the **relay protocol**, but not a binding to
this GCP service:

1. Oko Desktop calls `reclaimBrokerSessions()` at startup and every 60 seconds.
2. Publisher activation flows from
   `Settings.resolvedPTYRelayURL` and `resolvedPTYRelayToken` into
   `Workspace.reclaimBrokerSessions`.
3. `Workspace+TeamShared.swift` returns without publishing when the URL is
   empty, the token is empty, or the URL is not a normalized WebSocket URL.
4. When configured, `PTYRelayBridge.publish` connects as `role=publisher`;
   Oko Desktop control actions connect as `role=control`.
5. A viewer can also obtain a relay URL dynamically from an active
   `oko_live_sessions.ssh_url` row and invoke `oko-cli pty relay-attach`, which
   connects as `role=viewer`.
6. No current Oko, Oko Desktop, Stado or Wisent Backend source/configuration
   contains either deployed Cloud Run URL. The only committed
   `STADO_PTY_RELAY_URL` value is empty/example configuration.

The current server contract also differs from the deployed artifact. Current
`oko/relay/pty-relay.ts` ignores `SWIATOWID_PTY_RELAY_TOKEN`; it launches the
local Stado CLI and resolves `oko-pty-relay/token` from Skarbiec as consumer
`oko-pty-relay-service`. The Cloud Run revision instead injects
`SWIATOWID_PTY_RELAY_TOKEN` from GCP Secret Manager and uses the default Compute
service account. Therefore the repository contains a possible Oko relay client
and a newer Stado-managed relay server, but no code/configuration evidence that
selects the deployed GCP service.

## Compute Engine: what is needed

### Instances

All 20 residual instances are `TERMINATED`; **zero GCP instances are needed**. The useful product capabilities either moved or need to be fixed on their declared local/Azure placements, not resurrected from terminated GCP machines.

### Disks

| Proposed classification | Disks | Why |
|---|---|---|
| `EXPORT-THEN-DELETE` | `content-platform-vm` | 500 GB recordings host; no proof that `~/weles/recordings` was copied. |
| `EXPORT-THEN-DELETE` | `needher-watermark-a100-80gb-20260622` | Possible unique NeedHer pipeline results/checkpoints. |
| `EXPORT-THEN-DELETE` | `vatt-a100` | Possible unique VATT experiment output. |
| `EXPORT-THEN-DELETE` | `sapiens2-bodyhorror` | Possible unique Sapiens2/NeedHer output. |
| `EXPORT-THEN-DELETE` | `wisent-zimage-interp-jt` | Possible unique Z-Image checkpoint/result data. |
| `DELETE` | `image-video-router-vm`, `weles-apple-auth-prod` | Runtime replaced/declared elsewhere; no unique-data evidence. |
| `DELETE` | thirteen `wisent-agent-*` / `wisent-<hex>` worker disks | Ephemeral Stado workers; individual machines and disks have no durable role. |

The need is for data from five disks, not for the disks or their GCP runtimes. The two `zimage-*` machine-image recovery artifacts can remain only until the associated Z-Image data export is verified.

### All other compute assets

| Family | Proposed classification | Reason |
|---|---|---|
| 13 instance groups and 13 managers | `DELETE` | No live member instance and no GCP service remains. |
| 165 instance templates and 38 instance settings | `DELETE` | Generated deployment history; source/configuration belongs in repositories and Stado. |
| 184 images | `DELETE` after any unique model/data extraction | Build artifacts, not canonical model storage. The current relay image is in Artifact Registry, not this family. |
| 12 backend services, five health checks, three forwarding rules, one backend bucket | `DELETE` | Serve absent backend/image/inference/training instances. |
| Three addresses | `DELETE` | `wisent-lb-ip`, `comfyui-static-ip` and `wisent-dev-ip` have no required GCP endpoint. |
| HTTP/HTTPS proxies, two URL maps and SSL certificate | `DELETE` | Old Wisent Backend load balancer. |
| 25 firewall rules | `DELETE` except Google-required defaults while the network exists | Old API, image, inference, ComfyUI, proxy, VNC, SSH/RDP and development access. |
| Two networks, 43 subnetworks, 45 routes and one router | `DELETE` after dependent resources | No Wisent GCP runtime remains; most subnets/routes are generated regional defaults. |

## Cloud Storage: all 17 proposed classifications

| Bucket | Proposed classification | Required content / criterion |
|---|---|---|
| `stado` | `DEDUP-THEN-DELETE` | Compare registry, releases, artifacts, queue history and Probierz records against Azure primary and S3 DR; export the two-object mismatch recorded during migration plus any later unique objects. |
| `wisent-compute` | `DEDUP-THEN-DELETE` | Preserve unique legacy registry, release, agent, schedule, log and run records; no current writer should remain. |
| `wisent-images-bucket` | `MIGRATE-THEN-CUTOVER` | Critical active Wisent app media: the 2026-08-17 manifest contains 2,674 database locators covering 3.011 GiB across 1,664 `Character.imageUrl`, 927 `Character.videoUrl`, 21 `ProfilePublic.imageUrl`, and 62 `Room.imageUrl` values. Every object name and source metadata record exists; body reads remain blocked by detached billing. Recover and copy every referenced body to Azure object storage behind Stado, verify the destination, replace raw provider URLs with the canonical delivery contract, verify application reads, and only then remove the GCP copy. |
| `wisent-gcp-pipeline` | `DEDUP-THEN-DELETE` | Preserve unique ComfyUI models/outputs, LoRAs, NeedHer work and SmoothMix artifacts. |
| `wisent-gcp-bucket` | `DEDUP-THEN-DELETE` | It is already described as a legacy mirror; compare against the two image/model buckets and canonical Stado storage before removal. |
| `kantbench-training` | `EXPORT-THEN-DELETE` | Preserve code only if absent from Git, plus unique checkpoints, evaluations, Optuna/hyperopt state and run evidence. |
| `wisent-body-horror-models` | `EXPORT-THEN-DELETE` | Preserve the useful Sapiens2 model material once; no live detector depends on the GCP bucket. |
| `wisent-stock-context` | `EXPORT-THEN-DELETE` | Archive the AAPL/ADBE/NOK/NVDA/ORCL context if it has research value; no active producer was found. |
| `wisent-video-gen` | `EXPORT-THEN-DELETE` | Archive the one Civitai exact-video result if wanted as product/research evidence. |
| `wisent-oko-updates` | `HOLD-COMPAT` | Keep only until installed copies on the historical feed can reach the GitHub/Stado appcast or fall below the supported version floor. No new release should write here. |
| `wisent-swiatowid-updates` | `HOLD-COMPAT` | Same compatibility hold for still older installations; no new release writer. |
| `run-sources-wisent-480400-europe-west1` | `DELETE` | Source staging for the unused Cloud Run relay; canonical source exists in Oko. |
| `wisent-480400-skarbiec-vault` | `DELETE` after current Skarbiec recovery is proven | An encrypted legacy vault copy is not an approved second source of truth and increases secret-retention surface. |
| `wisent-480400_cloudbuild` | `DELETE` | Old build-source staging; Git repositories are canonical. |
| `gcf-v2-sources-1080673333190-us-central1` | `DELETE` | Empty generated staging; no live Function. |
| `gcf-v2-uploads-1080673333190.us-central1.cloudfunctions.appspot.com` | `DELETE` | Empty generated staging; no live Function. |
| `wisent-jobs-wisent-480400` | `DELETE` | Empty legacy job bucket; no consumer. |

## Data services and messaging

| Resource | Proposed classification | Required preservation |
|---|---|---|
| BigQuery `billing_export.gcp_billing_export_v1_017364_D3B657_F207B5` | `OPTIONAL-ARCHIVE` | No current consumer: active Stado billing configuration selects only Azure. The table contains historical GCP gross cost, credits, net cost and burn data; export it only if that financial history is wanted. |
| Cloud SQL `wisent-compute-db` | `EXPORT-THEN-DELETE` | Schema and data once, unless inspection proves the database empty/disposable; never restart it merely to preserve the old runtime. |
| Pub/Sub `stado-alerts` | `DELETE` | Current Stado alerting must use its configured current channel; this topic has no subscription. |
| Pub/Sub `wisent-compute-alerts` | `DELETE` | Retired coordinator topic; no subscription. |
| Pub/Sub `wisent-job-alerts` | `DELETE` | Historical monitor topic; no subscription. |
| Pub/Sub `Play-Store-Notifications` | `DELETE` after external publisher target is changed/removed | No subscriber means the GCP topic is not a working ingestion path. |
| Cloud Tasks `bodyhorror-detector` | `DELETE` | No detector runtime consumes the queue. |
| Three Storage Transfer jobs | `DELETE` after recording metadata | Old iOS Qwen restore and two unidentified transfer definitions are not runtime capabilities. |

## Identities and credentials

### Service accounts

| Account | Proposed classification |
|---|---|
| `1080673333190-compute@developer.gserviceaccount.com` (Default Compute) | `DELETE` after confirming no Google-managed dependency beyond the unused relay; its user-managed key is unnecessary. |
| `agent-billing` | `DELETE` after billing export; no ongoing GCP billing feed. |
| `brama-runtime` | `DELETE`; Brama does not belong in GCP. |
| `stado-sa`, `wisent-compute-sa`, `wisent-monitor` | `DELETE`; old GCP Stado plane. |
| `asc-webhook-bridge` | `DELETE`; bridge service absent. |
| `bodyhorror-detector` | `DELETE`; detector service absent. |
| `claude-pr-reviewer` | `DELETE`; direct Vertex path violates the Brama boundary and no current core workflow reference was found. |
| `kantbench-training` | `DELETE` after training data export; future model work uses Stado-selected capacity and Brama where inference is required. |
| `revenuecat-service-account` | `DELETE` with the orphan topic or replace the integration outside GCP first. |
| `wisent-images-sa` | `DELETE`; old GCP image runtime. |
| App Engine default account | `DELETE`/`CASCADE`; no App Engine workload. |
| `droid-441` | `DELETE` after retirement operations; three keys and project Owner make it the highest-risk residual identity. |

The twelve user-managed service-account keys are not needed as data. Revoke/delete them as their accounts leave service; never export them as migration artifacts.

### Workload Identity Federation

`github-pool` and `github-provider` are `DELETE`. No current GCP authentication or deployment reference was found in the core Stado, Wisent Backend, Oko or Oko Desktop workflows; current source ownership is GitHub, while deployment belongs through Stado rather than direct GCP workflow credentials.

### API keys

- `translate-i18n`: `DELETE`; no current consumer evidence.
- Both `gemini-image-gen` keys: `DELETE`; model calls belong through Brama and the keys lack API restrictions.

### Secret Manager

| Secret group | Proposed classification |
|---|---|
| `swiatowid-pty-relay-token` | `DELETE`; no request consumer was observed and the Cloud Run runtime cannot fetch it with billing detached. A future Stado relay must use separate exact Skarbiec service/client grants. |
| `account-api-env`, `wisent-api-env`, `wisent-images-env`, `wisent-images-supabase-key`, `wisent-images-supabase-service-role-key`, `wisent-images-supabase-url`, `wisent-inference-env`, `wisent-inference-env-bf16-a10080` | `DELETE` after verifying current services receive every still-valid value through Skarbiec; do not copy obsolete provider/runtime configuration forward. |
| `hf-token`, `wisent-hf-token`, `wandb-api-key`, `vast-api-key` | `DELETE`; re-materialize only a credential that an active Stado workload explicitly requires. |
| `asc-webhook-secret`, `github-dispatch-token` | `DELETE` with the absent bridge; rotate any external webhook/dispatch credential that remains valid. |
| `brama-skarbiec-gpg-private-key` | `DELETE`; GCP must not retain Brama/Skarbiec private material. |
| `supabase-access-token` | `DELETE` after confirming current deployment automation uses Skarbiec. |
| `wisent-gh-token` | `DELETE` after confirming current Stado bootstrap/release access uses Skarbiec or GitHub App identity. |
| `wisent-local-bmc-redfish` | `DELETE`; a local workstation BMC credential does not belong in GCP. |

The 132 secret versions are history of these 19 secrets, not 132 separately needed values.

## Build, monitoring and administrative resources

| Family | Proposed classification | Reason |
|---|---|---|
| Artifact Registry `cloud-run-source-deploy` | `DELETE`; the relay is unused/nonfunctional and the backend image is historical. |
| Seven Cloud Run revisions | `DELETE`/`CASCADE` | No request traffic was observed; runtime startup aborts on its secret read. |
| Four log metrics and four alert policies | `DELETE` | They monitor the retired GCP Stado scheduler/worker lifecycle. |
| Notification channel `wisent-compute-alerts-email` | `DELETE` | Only supports those retired policies. |
| Two log buckets and two sinks | `CASCADE`/retain only as Google-required audit defaults while the project exists | Not Wisent product services. |
| Ten Dataplex entry groups | `CASCADE` | Automatic metadata for data products, not workloads. |
| Essential Contact | Keep with administrative tombstone | Administrative notification, not runtime. |
| Org Policy `iam.allowedPolicyMemberDomains` | Keep with administrative tombstone | Security boundary on the retained project. |
| Connectivity Test `ssh-troubleshoot-cmh25` | `DELETE` | One historical diagnostic. |
| Billing info record | Keep as closed tombstone | Documents intentionally detached billing. |
| 119 quota preferences | `DELETE`/`CASCADE` | They are requests, not capacity; no GCP GPU/CPU fleet is planned. |
| Project asset | Keep as tombstone initially | Delete the project only after all exports, compatibility holds and security cleanup are complete; project deletion is not required to remove GCP from the architecture. |

## Enabled APIs

No enabled API is a permanent product dependency. Keep only the APIs required to complete the current phase:

| Phase | APIs still operationally needed |
|---|---|
| Data export remains | Storage, Compute, BigQuery, SQL Admin, Cloud Asset and IAM/service APIs needed to read/export/delete the named resources. |
| Compatibility feeds remain | Cloud Storage serving/administration APIs. |
| Tombstone | Only Google-required project/Service Usage/Resource Manager and administrative visibility; disable every optional product API. |

Workspace, Ads, Android Publisher, Translate, Gemini/Vertex and the many enabled platform/catalog APIs are not evidence of GCP-hosted workloads and do not justify retaining GCP resources.

## Proposed retirement order encoded by dependency

1. Recover and cut over the live `wisent-images-bucket` objects and all 2,362 database locators according to the Microsoft migration strategy.
2. Establish Azure-backed Stado state and new media writes before moving historical control-plane data.
3. Preserve the five unique disks and export/deduplicate the remaining named buckets, BigQuery table and Cloud SQL contents.
4. Keep the two update buckets only for measured old-client compatibility; stop every new writer.
5. Remove unused compute/service infrastructure only after required capabilities work at their declared replacements.
6. Revoke all twelve service-account keys, retire the fourteen accounts as classified, remove WIF, API keys and GCP Secret Manager values, and disable optional APIs.
7. Leave `wisent-480400` as a billing-detached tombstone until export checks and retention obligations pass; deletion of the whole project is optional and separate.

## Coverage

This assessment classifies every family in the 1,057-asset inventory:

- all 599 Compute Engine assets;
- all 109 non-compute logical/support resources across 27 asset families;
- all 86 enabled APIs;
- all 119 quota preferences;
- all 132 secret versions under the 19 secrets;
- all 12 user-managed service-account keys.

Source inventory: [`gcp-service-inventory-2026-08-15.md`](gcp-service-inventory-2026-08-15.md). Approved sequencing and Microsoft targets: [`gcp-to-microsoft-migration-strategy-2026-08-15.md`](gcp-to-microsoft-migration-strategy-2026-08-15.md). Exact instance/disk topology: [`gcp-compute-retirement-2026-08-15.json`](gcp-compute-retirement-2026-08-15.json).
