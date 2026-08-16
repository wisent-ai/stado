# GCP resource necessity map — 2026-08-15

## Decision

**No Wisent capability needs to remain permanently in GCP.** The intended terminal state for project `wisent-480400` is a retained administrative tombstone with billing detached and no Wisent workload, credential, public endpoint or unique data left inside it.

Three things still require preservation before that state is safe:

1. **One temporary public compatibility path:** the Oko internet PTY relay currently represented by Cloud Run `swiatowid-pty-relay`, until an equivalent single-instance relay is deployed through Stado and client routing is cut over.
2. **Legacy desktop update compatibility:** the two public Sparkle feeds, until old Oko/Swiatowid installations have a proven path to the GitHub/Stado feed.
3. **Unique or unclassified data:** five persistent disks, selected buckets, historical billing data and the stopped Cloud SQL database, until their useful contents are exported or deliberately discarded after inspection.

Everything else is either already replaced, an inert deployment artifact, a credential for a retired path, metadata generated around another resource, or an API/quota declaration that no workload needs.

## Meaning of the decisions

| Decision | Meaning |
|---|---|
| `KEEP-UNTIL-CUTOVER` | A capability is still useful and the GCP resource may be the only deployed implementation; replace it before removal. |
| `HOLD-COMPAT` | No new system should use it, but old installed clients may still address it. |
| `EXPORT-THEN-DELETE` | The data may matter; the GCP runtime/resource does not. Export, verify the destination and remove the source. |
| `DEDUP-THEN-DELETE` | Likely replicated or historical data; compare with canonical storage, preserve only unique objects and remove the GCP copy. |
| `DELETE` | No current capability or unique-data requirement justifies retention. |
| `CASCADE` | Google-generated metadata/support object; let deletion of its parent remove it or remove it after the parent. |

`DELETE` is a necessity decision, not evidence that the current detached-billing control plane will accept the deletion command.

## Capability-level map

| Capability | Needed by Wisent | Needed in GCP | Decision |
|---|---|---|---|
| Stado coordinator, queue, object and release APIs | Yes | No | Already placed on `charless-mac-mini` with Azure Blob primary storage and S3 DR; GCP copies are migration/history data only. |
| Brama model routing | Yes | No | Brama owns model access; old GCP identities, Vertex configuration and direct Gemini keys are not a valid route. |
| Wisent Backend public API | Yes | No | Replaced on `charless-mac-mini`; all API MIG/LB/template/image assets are deletable. |
| Echo and Weles durable worker duties | Yes | No | Replaced on `charless-mac-mini`; preserve only unexported recordings from the old disk. |
| Weles Apple authentication | Yes | No | Replaced on `charless-mac-mini`; old VM and disk are deletable. |
| Oko internet PTY relay | Yes | Temporarily | Current Oko code requires one Stado-managed relay instance. Cloud Run is Ready and supplies the only observed public relay implementation, but the current local Oko preference has no configured relay URL. Keep the GCP relay stack only until the Stado-managed `oko-pty-relay` endpoint and client configuration are proven. |
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

## Temporary GCP dependency: PTY relay

The GCP relay is one dependency stack, not seven separate services:

| Resource | Necessity |
|---|---|
| Cloud Run `swiatowid-pty-relay` | `KEEP-UNTIL-CUTOVER` |
| Latest revision `swiatowid-pty-relay-00007-ctx` | `KEEP-UNTIL-CUTOVER`; older six revisions are `CASCADE`/`DELETE` |
| Latest relay image digest in `cloud-run-source-deploy` | `KEEP-UNTIL-CUTOVER`; older relay digests are `DELETE` |
| `gs://run-sources-wisent-480400-europe-west1` relay source object | `KEEP-UNTIL-CUTOVER`; source also exists in current Oko code, so it is not a durable archive requirement |
| Secret `swiatowid-pty-relay-token` | `KEEP-UNTIL-CUTOVER`; replacement belongs in Skarbiec with exact service/client grants |
| Default Compute service account | `KEEP-UNTIL-CUTOVER`, scoped only because Cloud Run currently uses it; retire its user-managed key and account dependency at cutover |
| Run, Artifact Registry, Secret Manager, IAM, logging and monitoring APIs | Needed only to operate/retire this relay |

The replacement contract is already explicit in Oko: one long-lived Stado-managed `oko-pty-relay`, TLS, exact Skarbiec grants, and one instance because session pairing is in process memory.

## Compute Engine: what is needed

### Instances

All 20 residual instances are `TERMINATED`; **zero GCP instances are needed**. The useful product capabilities either moved or need to be fixed on their declared local/Azure placements, not resurrected from terminated GCP machines.

### Disks

| Decision | Disks | Why |
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

| Family | Decision | Reason |
|---|---|---|
| 13 instance groups and 13 managers | `DELETE` | No live member instance and no GCP service remains. |
| 165 instance templates and 38 instance settings | `DELETE` | Generated deployment history; source/configuration belongs in repositories and Stado. |
| 184 images | `DELETE` after any unique model/data extraction | Build artifacts, not canonical model storage. The current relay image is in Artifact Registry, not this family. |
| 12 backend services, five health checks, three forwarding rules, one backend bucket | `DELETE` | Serve absent backend/image/inference/training instances. |
| Three addresses | `DELETE` | `wisent-lb-ip`, `comfyui-static-ip` and `wisent-dev-ip` have no required GCP endpoint. |
| HTTP/HTTPS proxies, two URL maps and SSL certificate | `DELETE` | Old Wisent Backend load balancer. |
| 25 firewall rules | `DELETE` except Google-required defaults while the network exists | Old API, image, inference, ComfyUI, proxy, VNC, SSH/RDP and development access. |
| Two networks, 43 subnetworks, 45 routes and one router | `DELETE` after dependent resources | No Wisent GCP runtime remains; most subnets/routes are generated regional defaults. |

## Cloud Storage: all 17 decisions

| Bucket | Decision | Required content / criterion |
|---|---|---|
| `stado` | `DEDUP-THEN-DELETE` | Compare registry, releases, artifacts, queue history and Probierz records against Azure primary and S3 DR; export the two-object mismatch recorded during migration plus any later unique objects. |
| `wisent-compute` | `DEDUP-THEN-DELETE` | Preserve unique legacy registry, release, agent, schedule, log and run records; no current writer should remain. |
| `wisent-images-bucket` | `DEDUP-THEN-DELETE` | Preserve unique generated media, character assets, LoRAs, checkpoints, activations and datasets in canonical object/model storage. |
| `wisent-gcp-pipeline` | `DEDUP-THEN-DELETE` | Preserve unique ComfyUI models/outputs, LoRAs, NeedHer work and SmoothMix artifacts. |
| `wisent-gcp-bucket` | `DEDUP-THEN-DELETE` | It is already described as a legacy mirror; compare against the two image/model buckets and canonical Stado storage before removal. |
| `kantbench-training` | `EXPORT-THEN-DELETE` | Preserve code only if absent from Git, plus unique checkpoints, evaluations, Optuna/hyperopt state and run evidence. |
| `wisent-body-horror-models` | `EXPORT-THEN-DELETE` | Preserve the useful Sapiens2 model material once; no live detector depends on the GCP bucket. |
| `wisent-stock-context` | `EXPORT-THEN-DELETE` | Archive the AAPL/ADBE/NOK/NVDA/ORCL context if it has research value; no active producer was found. |
| `wisent-video-gen` | `EXPORT-THEN-DELETE` | Archive the one Civitai exact-video result if wanted as product/research evidence. |
| `wisent-oko-updates` | `HOLD-COMPAT` | Keep only until installed copies on the historical feed can reach the GitHub/Stado appcast or fall below the supported version floor. No new release should write here. |
| `wisent-swiatowid-updates` | `HOLD-COMPAT` | Same compatibility hold for still older installations; no new release writer. |
| `run-sources-wisent-480400-europe-west1` | `KEEP-UNTIL-CUTOVER` | Only for the current Cloud Run relay; delete with it. |
| `wisent-480400-skarbiec-vault` | `DELETE` after current Skarbiec recovery is proven | An encrypted legacy vault copy is not an approved second source of truth and increases secret-retention surface. |
| `wisent-480400_cloudbuild` | `DELETE` | Old build-source staging; Git repositories are canonical. |
| `gcf-v2-sources-1080673333190-us-central1` | `DELETE` | Empty generated staging; no live Function. |
| `gcf-v2-uploads-1080673333190.us-central1.cloudfunctions.appspot.com` | `DELETE` | Empty generated staging; no live Function. |
| `wisent-jobs-wisent-480400` | `DELETE` | Empty legacy job bucket; no consumer. |

## Data services and messaging

| Resource | Decision | Required preservation |
|---|---|---|
| BigQuery `billing_export.gcp_billing_export_v1_017364_D3B657_F207B5` | `EXPORT-THEN-DELETE` | Historical gross cost, credits, net cost and burn history as a portable table/archive. |
| Cloud SQL `wisent-compute-db` | `EXPORT-THEN-DELETE` | Schema and data once, unless inspection proves the database empty/disposable; never restart it merely to preserve the old runtime. |
| Pub/Sub `stado-alerts` | `DELETE` | Current Stado alerting must use its configured current channel; this topic has no subscription. |
| Pub/Sub `wisent-compute-alerts` | `DELETE` | Retired coordinator topic; no subscription. |
| Pub/Sub `wisent-job-alerts` | `DELETE` | Historical monitor topic; no subscription. |
| Pub/Sub `Play-Store-Notifications` | `DELETE` after external publisher target is changed/removed | No subscriber means the GCP topic is not a working ingestion path. |
| Cloud Tasks `bodyhorror-detector` | `DELETE` | No detector runtime consumes the queue. |
| Three Storage Transfer jobs | `DELETE` after recording metadata | Old iOS Qwen restore and two unidentified transfer definitions are not runtime capabilities. |

## Identities and credentials

### Service accounts

| Account | Decision |
|---|---|
| `1080673333190-compute@developer.gserviceaccount.com` (Default Compute) | `KEEP-UNTIL-CUTOVER` only for the relay; remove its user-managed key now if Cloud Run does not require it, then retire the account dependency with the relay. |
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

| Secret group | Decision |
|---|---|
| `swiatowid-pty-relay-token` | `KEEP-UNTIL-CUTOVER`, then replace with the exact `oko-pty-relay` Skarbiec service/client grants and delete both GCP versions. |
| `account-api-env`, `wisent-api-env`, `wisent-images-env`, `wisent-images-supabase-key`, `wisent-images-supabase-service-role-key`, `wisent-images-supabase-url`, `wisent-inference-env`, `wisent-inference-env-bf16-a10080` | `DELETE` after verifying current services receive every still-valid value through Skarbiec; do not copy obsolete provider/runtime configuration forward. |
| `hf-token`, `wisent-hf-token`, `wandb-api-key`, `vast-api-key` | `DELETE`; re-materialize only a credential that an active Stado workload explicitly requires. |
| `asc-webhook-secret`, `github-dispatch-token` | `DELETE` with the absent bridge; rotate any external webhook/dispatch credential that remains valid. |
| `brama-skarbiec-gpg-private-key` | `DELETE`; GCP must not retain Brama/Skarbiec private material. |
| `supabase-access-token` | `DELETE` after confirming current deployment automation uses Skarbiec. |
| `wisent-gh-token` | `DELETE` after confirming current Stado bootstrap/release access uses Skarbiec or GitHub App identity. |
| `wisent-local-bmc-redfish` | `DELETE`; a local workstation BMC credential does not belong in GCP. |

The 132 secret versions are history of these 19 secrets, not 132 separately needed values.

## Build, monitoring and administrative resources

| Family | Decision | Reason |
|---|---|---|
| Artifact Registry `cloud-run-source-deploy` | `KEEP-UNTIL-CUTOVER` only for the latest relay image; delete the old relay and backend images, then the repository. |
| Seven Cloud Run revisions | Keep latest only until cutover; six older revisions `DELETE` | Rollback history is not canonical source. |
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
| Relay remains | Run, Artifact Registry, Secret Manager, IAM, logging/monitoring and supporting Service Usage APIs. |
| Data export remains | Storage, Compute, BigQuery, SQL Admin, Cloud Asset and IAM/service APIs needed to read/export/delete the named resources. |
| Compatibility feeds remain | Cloud Storage serving/administration APIs. |
| Tombstone | Only Google-required project/Service Usage/Resource Manager and administrative visibility; disable every optional product API. |

Workspace, Ads, Android Publisher, Translate, Gemini/Vertex and the many enabled platform/catalog APIs are not evidence of GCP-hosted workloads and do not justify retaining GCP resources.

## Retirement order encoded by dependency

1. Preserve the five unique disks and export/deduplicate the named buckets, BigQuery table and Cloud SQL contents.
2. Deploy the single-instance Oko relay through Stado with Skarbiec grants; cut clients from the Cloud Run URL and token.
3. Preserve legacy Sparkle compatibility by moving or redirecting the two old feeds; stop all GCP release writes.
4. Remove the relay stack, all terminated compute support assets, orphan messaging/data services and obsolete build/monitoring resources.
5. Revoke all twelve service-account keys, retire the fourteen accounts as classified, remove WIF, API keys and GCP Secret Manager values, and disable optional APIs.
6. Leave `wisent-480400` as a billing-detached tombstone until export checks and retention obligations pass; deletion of the whole project is optional and separate.

## Coverage

This decision map classifies every family in the 1,057-asset inventory:

- all 599 Compute Engine assets;
- all 109 non-compute logical/support resources across 27 asset families;
- all 86 enabled APIs;
- all 119 quota preferences;
- all 132 secret versions under the 19 secrets;
- all 12 user-managed service-account keys.

Source inventory: [`gcp-service-inventory-2026-08-15.md`](gcp-service-inventory-2026-08-15.md). Exact instance/disk topology: [`gcp-compute-retirement-2026-08-15.json`](gcp-compute-retirement-2026-08-15.json).
