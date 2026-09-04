//! Versioned configuration accessors and operational constants.
//!
//! Environment variables are limited to documented route-local overrides.
//! Deployment-wide provider, storage, identity, and policy state resolves from
//! the selected schema-versioned configuration file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{LazyLock, RwLock};
use std::time::Instant;

use crate::catalog::GPU_SIZING;
use crate::config_file::{expand_tilde, resolve as cfg, resolve_list as cfg_list};

use serde_json::Value;

fn resolve_binding(
    field: &crate::capabilities::ConfigField,
    backup: bool,
    default: &str,
) -> String {
    let (env, path, fallback) = if backup {
        (
            field
                .backup_env
                .expect("catalog field has no backup environment binding"),
            field
                .backup_path
                .expect("catalog field has no backup configuration path"),
            default.to_string(),
        )
    } else {
        let fallback = if field.fallback_env.is_some() || field.fallback_path.is_some() {
            cfg(
                field.fallback_env.unwrap_or(""),
                field.fallback_path.unwrap_or(""),
                default,
            )
        } else {
            default.to_string()
        };
        (field.env, field.path, fallback)
    };
    cfg(env, path, &fallback)
}

fn resolve_capability_binding(
    kind: crate::capabilities::RuntimeFacet,
    variant: &str,
    key: &str,
    backup: bool,
    default: &str,
) -> String {
    let field = crate::capabilities::config_field(kind, variant, key)
        .expect("runtime configuration binding is missing from the capability catalog");
    debug_assert_eq!(
        field.value_kind,
        crate::capabilities::ConfigValueKind::Scalar
    );
    resolve_binding(field, backup, default)
}

fn resolve_capability_list_binding(
    kind: crate::capabilities::RuntimeFacet,
    variant: &str,
    key: &str,
    default: &[&str],
) -> Vec<String> {
    let field = crate::capabilities::config_field(kind, variant, key)
        .expect("runtime list binding is missing from the capability catalog");
    debug_assert_eq!(field.value_kind, crate::capabilities::ConfigValueKind::List);
    cfg_list(field.env, field.path, default)
}

fn resolve_compute_binding(
    provider: crate::capabilities::ProviderId,
    key: &str,
    default: &str,
) -> String {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        provider.as_str(),
        key,
        false,
        default,
    )
}

fn resolve_compute_list_binding(
    provider: crate::capabilities::ProviderId,
    key: &str,
    default: &[&str],
) -> Vec<String> {
    resolve_capability_list_binding(
        crate::capabilities::RuntimeFacet::Compute,
        provider.as_str(),
        key,
        default,
    )
}

fn resolve_storage_binding(
    adapter: crate::capabilities::StorageAdapter,
    key: &str,
    backup: bool,
    default: &str,
) -> String {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Storage,
        adapter.id(),
        key,
        backup,
        default,
    )
}

const DEFAULT_GCP_PROJECT: &str = "";
const DEFAULT_GCS_BUCKET: &str = "";
const DEFAULT_GCP_REGION: &str = "us-central1";
const DEFAULT_GCP_REGIONS: &[&str] = &[
    "us-central1",
    "europe-west4",
    "us-east1",
    "us-east4",
    "us-east5",
];
const DEFAULT_PROVIDERS: &[&str] = &[];
const DEFAULT_STORAGE_BACKEND: &str = "";

fn resolve_storage_backend(backup: bool) -> String {
    let name = resolve_binding(
        &crate::capabilities::STORAGE_BACKEND_CONFIG,
        backup,
        DEFAULT_STORAGE_BACKEND,
    );
    crate::capabilities::canonical_id(crate::capabilities::RuntimeFacet::Storage, &name)
        .unwrap_or(&name)
        .to_string()
}

static PROJECT: LazyLock<String> = LazyLock::new(|| {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "project",
        false,
        DEFAULT_GCP_PROJECT,
    )
});
static BUCKET: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::Gcs,
        "bucket",
        false,
        DEFAULT_GCS_BUCKET,
    )
});
static REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "region",
        false,
        DEFAULT_GCP_REGION,
    )
});
static ALERTS_TOPIC: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_ALERTS_TOPIC")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            let default = if project().is_empty() {
                String::new()
            } else {
                format!("projects/{}/topics/stado-alerts", project())
            };
            cfg("", "alerts.topic", &default)
        })
});

static ALERT_CHANNELS: LazyLock<Vec<String>> =
    LazyLock::new(|| cfg_list("STADO_ALERT_CHANNELS", "alerts.channels", &[]));

/// Where email alerts go. The destination is not a secret, so it belongs in
/// the config document rather than the vault; the env name matches the one
/// the SendGrid channel has always read.
static ALERT_EMAIL_TO: LazyLock<String> =
    LazyLock::new(|| cfg("WC_EMAIL_TO", "alerts.email_to", ""));

/// Sender for email alerts; must be a domain the provider has verified.
static ALERT_EMAIL_FROM: LazyLock<String> =
    LazyLock::new(|| cfg("WC_EMAIL_FROM", "alerts.email_from", ""));

/// Vault item holding the Resend API key, and the field inside it. A
/// deployment's live key is not always in the item a default would guess:
/// this one keeps it in the Weles management item, while `RESEND_API_KEY`
/// holds a key the provider has already rejected.
static ALERT_RESEND_ITEM: LazyLock<String> =
    LazyLock::new(|| cfg("WC_RESEND_ITEM", "alerts.resend_item", "RESEND_API_KEY"));

static ALERT_RESEND_FIELD: LazyLock<String> =
    LazyLock::new(|| cfg("WC_RESEND_FIELD", "alerts.resend_field", "value"));

/// GCP project id (env `GCP_PROJECT`).
pub fn project() -> &'static str {
    PROJECT.as_str()
}

/// Queue storage bucket (env `WC_BUCKET`).
pub fn bucket() -> &'static str {
    BUCKET.as_str()
}

/// Primary GCP region (env `GCP_REGION`).
pub fn region() -> &'static str {
    REGION.as_str()
}

/// Pub/Sub alerts topic (env `WC_ALERTS_TOPIC`).
pub fn alerts_topic() -> &'static str {
    ALERTS_TOPIC.as_str()
}

/// Explicitly enabled optional alert adapters.
pub fn alert_channels() -> &'static [String] {
    ALERT_CHANNELS.as_slice()
}

/// Destination for email alert channels (env `WC_EMAIL_TO`).
pub fn alert_email_to() -> &'static str {
    ALERT_EMAIL_TO.as_str()
}

/// Sender for email alert channels (env `WC_EMAIL_FROM`).
pub fn alert_email_from() -> &'static str {
    ALERT_EMAIL_FROM.as_str()
}

/// Vault item holding the Resend API key (env `WC_RESEND_ITEM`).
pub fn alert_resend_item() -> &'static str {
    ALERT_RESEND_ITEM.as_str()
}

/// Field inside [`alert_resend_item`] (env `WC_RESEND_FIELD`).
pub fn alert_resend_field() -> &'static str {
    ALERT_RESEND_FIELD.as_str()
}

static REGIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    resolve_capability_list_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "regions",
        DEFAULT_GCP_REGIONS,
    )
});

/// Multi-region dispatch (env `GCP_REGIONS`, comma-separated). Every region
/// listed here is queried for live quota AND iterated by the GCP provider
/// when creating instances. Each region carries a default GCP-issued quota
/// (16 preemptible A100, 4 preemptible A100-80GB, 8 preemptible L4, 8
/// preemptible T4) so spreading across these 5 regions lifts total
/// parallel-VM ceiling from ~28 to ~140 without any quota-increase request.
/// Override with GCP_REGIONS=us-central1,europe-west4 (comma-separated) to
/// narrow the dispatch surface for testing.
pub fn regions() -> &'static [String] {
    &REGIONS
}

static ZONE_ROTATION: LazyLock<Vec<String>> = LazyLock::new(|| {
    let region = region();
    let mut zones = vec![
        format!("{region}-b"),
        format!("{region}-a"),
        format!("{region}-c"),
        format!("{region}-f"),
    ];
    zones.extend(
        [
            "europe-west4-a",
            "europe-west4-b",
            "europe-west4-c",
            "us-east1-c",
            "us-east1-d",
            "us-east4-a",
            "us-east4-b",
            "us-east4-c",
            "us-east5-a",
            "us-east5-b",
            "us-east5-c",
        ]
        .into_iter()
        .map(str::to_string),
    );
    zones
});

/// Zones, ordered by preference. Primary region's zones first (lowest
/// egress from existing infra in us-central1), then alternates. Provider
/// iterates this list and falls through GCE 'does not exist' / 'no
/// capacity' errors until one zone accepts the create_instance call.
pub fn zone_rotation() -> &'static [String] {
    &ZONE_ROTATION
}

static MACHINE_TYPE_ZONES: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(|| {
    let region = region();
    HashMap::from([
        (
            "a2-ultragpu-1g".to_string(),
            vec![
                format!("{region}-c"),
                format!("{region}-a"),
                "us-east5-a".to_string(),
                "us-east5-b".to_string(),
                "europe-west4-a".to_string(),
                // Removed 2026-05-01: machine-type not present in
                // europe-west4-b; NVIDIA_A100_80GB_GPUS regional quota is 0
                // in us-east4, so us-east4-c was generating "Quota exceeded"
                // errors every tick.
            ],
        ),
        (
            "a2-highgpu-1g".to_string(),
            vec![
                format!("{region}-b"),
                format!("{region}-a"),
                format!("{region}-c"),
                format!("{region}-f"),
                "europe-west4-a".to_string(),
                "europe-west4-b".to_string(),
                "us-east1-b".to_string(),
                // us-east1-c, us-east4-a, us-east4-b removed 2026-05-01:
                // confirmed via
                // `gcloud compute machine-types describe a2-highgpu-1g --zone=...`
                // that the SKU is not present in those zones; the dispatcher
                // was logging "Machine type does not exist" on every attempt
                // against them which wasted Cloud Function ticks and slowed
                // fleet ramp-up.
            ],
        ),
        (
            // nvidia-l4
            "g2-standard-4".to_string(),
            vec![
                format!("{region}-a"),
                format!("{region}-b"),
                format!("{region}-c"),
                "europe-west4-a".to_string(),
                "europe-west4-b".to_string(),
                "us-east1-c".to_string(),
                "us-east1-d".to_string(),
                "us-east4-a".to_string(),
                "us-east4-c".to_string(),
                // Removed 2026-05-01: g2-standard-4 not present in
                // us-east4-b, us-east5-a, us-east5-b. Confirmed via gcloud
                // compute machine-types describe; the dispatcher was logging
                // "Invalid machine type" each tick for these zones.
            ],
        ),
    ])
});

/// Per-machine-type zone rotation. Some SKUs don't exist in every zone, or
/// have regional spot-capacity quirks. For those buckets, list the zones
/// that actually carry the SKU first; the provider falls back to
/// [`zone_rotation`].
pub fn machine_type_zones() -> &'static HashMap<String, Vec<String>> {
    &MACHINE_TYPE_ZONES
}

pub const HEARTBEAT_STALE_MINUTES: i64 = 15;
pub const MAX_SCHEDULE_PER_TICK: i64 = 4;
pub const INSTANCE_PREFIX: &str = "wisent";

/// Defaults for the smart-routing CLI flags. 0 means "no cap"; the
/// scheduler only enforces a cost gate when this is positive.
pub const DEFAULT_MAX_COST_PER_HOUR_USD: f64 = 0.0;
pub const DEFAULT_PRIORITY: i64 = 0;
pub const DEFAULT_PREEMPTIBLE: bool = false;
pub const DEFAULT_ANY_PROVIDER: bool = true;

// --- Autonomous failure-fixer defaults ---
/// After this many fix attempts on the same job_id, the fixer stops
/// dispatching new Claude Code sessions so a permanently-broken job does
/// not burn unlimited subscription budget.
pub const FAILURE_FIXER_ATTEMPT_CAP: i64 = 3;
/// Per-job state-file prefix under BUCKET.
pub const FAILURE_FIXER_STATE_PREFIX: &str = "failure_fixes";
/// Max characters of failed/<jid>.json error field included in the
/// dispatched fix prompt. Big enough for Claude to see the full stack.
pub const FAILURE_FIX_PROMPT_ERROR_BYTES: i64 = 4000;
/// Seconds between failure-fixer scan_and_dispatch iterations when the
/// LaunchAgent runs in tight loop.
pub const FAILURE_FIXER_TICK_SECONDS: i64 = 180;
/// Command substring the LaunchAgent passes to wc-fix scan-dispatch
/// --command-pattern. Empty string means scan every failed/ blob, which
/// exhausts Claude Code subscription quota fast (live failure 2026-05-22:
/// 273 dispatches in one tick burned the daily limit). Set this to the
/// workload the operator wants the autonomous fixer to target.
/// raw.extract_and_upload is the canonical activation extraction workload.
pub const FAILURE_FIXER_COMMAND_PATTERN: &str = "raw.extract_and_upload";
/// Max fully-terminal runs the by-run reaper deletes per coordinator tick.
/// Bounds per-tick GCS work so a large backlog drains over several ticks
/// instead of one multi-thousand-blob delete stalling the tick.
pub const RUN_REAP_PER_TICK: i64 = 50;
/// Queue blobs the priority-marker index repair examines per coordinator
/// tick. The sweep is the standing repair for a queued job whose marker
/// write did not land — an unindexed job is invisible to every scheduler —
/// so it runs forever with a wrapping cursor rather than latching complete.
/// Bounds per-tick work to a names-only listing plus this many bodies.
pub const MARKER_REPAIR_PER_TICK: usize = 500;

// --- Coverage verifier + retry orchestrator defaults ---
/// After this many submit attempts on the same group_key the orchestrator
/// marks the tuple UNFIXABLE and stops retrying.
pub const COVERAGE_ATTEMPT_CAP: i64 = 5;
/// HTTP 429 backoff base; sleep = COVERAGE_VERIFY_BACKOFF_BASE ** attempt.
pub const COVERAGE_VERIFY_BACKOFF_BASE: i64 = 2;
/// Parallel verifier workers. Stays low to avoid HF rate-limit cap
/// (1000 requests / 300 s default).
pub const COVERAGE_VERIFY_THREADS: i64 = 4;
/// Stream progress every N entries during a verify walk.
pub const COVERAGE_PROGRESS_LOG_EVERY: i64 = 200;
/// GCS prefix under BUCKET for per-universe coverage state.
pub const COVERAGE_STATE_PREFIX: &str = "coverage";
/// Max retry-loop iterations for verify_request before re-raising 429.
pub const COVERAGE_HTTP_RETRY_CAP: i64 = 8;

fn cfg_i64(env_name: &str, dotted: &str, default: &str) -> i64 {
    cfg(env_name, dotted, default)
        .parse::<i64>()
        .unwrap_or_else(|_| panic!("{env_name} must be an integer"))
}

static DASHBOARD_BIND: LazyLock<String> =
    LazyLock::new(|| cfg("WC_DASHBOARD_BIND", "dashboard.bind", "127.0.0.1"));
static DASHBOARD_PORT: LazyLock<i64> =
    LazyLock::new(|| cfg_i64("WC_DASHBOARD_PORT", "dashboard.port", "8765"));
static DASHBOARD_TRUST_HTTPS_PROXY: LazyLock<bool> = LazyLock::new(|| {
    let value = cfg(
        "WC_DASHBOARD_TRUST_HTTPS_PROXY",
        "dashboard.trust_https_proxy",
        "false",
    );
    let value = value.trim();
    value == "1"
        || value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("yes")
        || value.eq_ignore_ascii_case("on")
});

/// API listener bind address (env `WC_DASHBOARD_BIND`). Azure
/// cutover keeps this on loopback behind the TLS reverse proxy; never bind a
/// private dashboard route directly to a public interface.
pub fn dashboard_bind() -> &'static str {
    DASHBOARD_BIND.as_str()
}

/// API listener port (env `WC_DASHBOARD_PORT`).
pub fn dashboard_port() -> i64 {
    *DASHBOARD_PORT
}

/// Whether the loopback listener accepts host authorities supplied by an HTTPS
/// reverse proxy.
pub fn dashboard_trust_https_proxy() -> bool {
    *DASHBOARD_TRUST_HTTPS_PROXY
}

/// Deployment identity (env `STADO_DEPLOYMENT_ID`, config
/// key `deployment.id`), trimmed. A bound deployment implies an authenticated
/// HTTPS reverse proxy fronts the loopback listener.
///
/// Read per call so a process-level override remains dynamic; the config
/// file itself is cached by [`crate::config_file`].
pub fn stado_deployment_id() -> String {
    cfg("STADO_DEPLOYMENT_ID", "deployment.id", "")
        .trim()
        .to_string()
}

pub const DEFAULT_IMAGE: &str = "pytorch-2-9-cu129-ubuntu-2204-nvidia-580-v20260408";
pub const DEFAULT_IMAGE_PROJECT: &str = "deeplearning-platform-release";
pub const DEFAULT_CPU_IMAGE_FAMILY: &str = "ubuntu-2204-lts";
pub const DEFAULT_CPU_IMAGE_PROJECT: &str = "ubuntu-os-cloud";
pub const DEFAULT_BOOT_DISK_GB: i64 = 200;

// Azure (parallel to GCP). All values resolved from env so the same
// wisent-compute install can target multiple subscriptions/resource groups
// without code changes. The provider does NOT create the vnet/subnet/NSG —
// it expects pre-provisioned infra named below.
static AZURE_SUBSCRIPTION_ID: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "subscription-id",
        "",
    )
});
static AZURE_RESOURCE_GROUP: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "resource-group",
        "wisent-compute",
    )
});
static AZURE_LOCATIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    resolve_compute_list_binding(
        crate::capabilities::ProviderId::Azure,
        "locations",
        &["eastus", "westus3", "westus2", "northeurope"],
    )
});
static AZURE_VNET: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "vnet",
        "wisent-compute-vnet",
    )
});
static AZURE_SUBNET: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "subnet",
        "wisent-compute-subnet",
    )
});
static AZURE_NSG: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "nsg",
        "wisent-compute-nsg",
    )
});
static AZURE_IMAGE_URN: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "image-urn",
        "microsoft-dsvm:ubuntu-hpc:2204:latest",
    )
});
static AZURE_VM_USERNAME: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Azure,
        "vm-username",
        "wisent",
    )
});
static AZURE_SSH_PUBLIC_KEY: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Azure, "ssh-public-key", "")
});
static AZURE_VM_IDENTITY_ID: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Azure, "vm-identity-id", "")
});

/// Azure subscription id (env `AZURE_SUBSCRIPTION_ID`).
pub fn azure_subscription_id() -> &'static str {
    AZURE_SUBSCRIPTION_ID.as_str()
}

/// Azure resource group (env `AZURE_RESOURCE_GROUP`).
pub fn azure_resource_group() -> &'static str {
    AZURE_RESOURCE_GROUP.as_str()
}

/// Azure dispatch locations (env `AZURE_LOCATIONS`, comma-separated).
pub fn azure_locations() -> &'static [String] {
    &AZURE_LOCATIONS
}

/// Pre-provisioned Azure vnet name (env `AZURE_VNET`).
pub fn azure_vnet() -> &'static str {
    AZURE_VNET.as_str()
}

/// Pre-provisioned Azure subnet name (env `AZURE_SUBNET`).
pub fn azure_subnet() -> &'static str {
    AZURE_SUBNET.as_str()
}

/// Pre-provisioned Azure NSG name (env `AZURE_NSG`).
pub fn azure_nsg() -> &'static str {
    AZURE_NSG.as_str()
}

/// Azure base image URN (env `AZURE_IMAGE_URN`,
/// publisher:offer:sku:version). microsoft-dsvm:ubuntu-hpc:2204:latest
/// ships with NVIDIA driver + CUDA preinstalled, matching
/// deeplearning-platform-release on GCP.
pub fn azure_image_urn() -> &'static str {
    AZURE_IMAGE_URN.as_str()
}

/// Azure cloud-init admin username (env `AZURE_VM_USERNAME`).
pub fn azure_vm_username() -> &'static str {
    AZURE_VM_USERNAME.as_str()
}

/// SSH public key for the cloud-init admin user (env
/// `AZURE_SSH_PUBLIC_KEY`). Required by Azure VM create even when SSH is
/// locked down via NSG; cloud-init will only accept the VM create call if
/// either ssh keys or password auth is configured.
pub fn azure_ssh_public_key() -> &'static str {
    AZURE_SSH_PUBLIC_KEY.as_str()
}

/// Resource id of the pre-provisioned user-assigned managed identity
/// attached to every agent VM (env `AZURE_VM_IDENTITY_ID`): a full ARM
/// path under
/// `.../providers/Microsoft.ManagedIdentity/userAssignedIdentities/`.
/// This is how the agent gets Azure credentials at all — on the VM the
/// token chain in [`crate::azure_token`] has no service-principal env
/// vars and no `az` CLI, so it falls through to IMDS, which answers only
/// for a VM that carries an identity. Empty (the default) emits no
/// identity block at VM create, leaving the agent unable to reach the
/// blob queue or to self-delete.
pub fn azure_vm_identity_id() -> &'static str {
    AZURE_VM_IDENTITY_ID.as_str()
}

// AWS uses the same catalog-driven env/config/default precedence as the other
// compute providers. The accessors remain LazyLock-backed because runtime
// configuration is immutable for the process lifetime.
static AWS_REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Aws, "region", "us-east-1")
});
static AWS_SECURITY_GROUP: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Aws, "security-group", "")
});
static AWS_IAM_PROFILE: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(
        crate::capabilities::ProviderId::Aws,
        "iam-profile",
        "stado-agent",
    )
});
static AWS_AMI_ID: LazyLock<String> = LazyLock::new(|| {
    resolve_compute_binding(crate::capabilities::ProviderId::Aws, "ami-id", "")
        .trim()
        .to_string()
});

/// AWS region for the EC2 provider (env `AWS_REGION`, default us-east-1).
pub fn aws_region() -> &'static str {
    AWS_REGION.as_str()
}

/// AWS security group id for agent instances (env `AWS_SECURITY_GROUP`).
/// Empty means "not configured" — the AWS provider refuses to create.
pub fn aws_security_group() -> &'static str {
    AWS_SECURITY_GROUP.as_str()
}

/// IAM instance profile name attached to agent instances (env
/// `AWS_IAM_PROFILE`, default "stado-agent").
pub fn aws_iam_profile() -> &'static str {
    AWS_IAM_PROFILE.as_str()
}

/// AMI id override (env `AWS_AMI_ID`, whitespace-stripped). Empty falls
/// back to the per-job image argument (Python
/// `os.environ.get("AWS_AMI_ID", "").strip() or image`).
pub fn aws_ami_id() -> &'static str {
    AWS_AMI_ID.as_str()
}

fn canonicalize_capability_names(
    kind: crate::capabilities::RuntimeFacet,
    values: Vec<String>,
) -> Vec<String> {
    values
        .into_iter()
        .map(|name| {
            crate::capabilities::canonical_id(kind, &name)
                .unwrap_or(&name)
                .to_string()
        })
        .collect()
}

static WC_DISABLED_PROVIDERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    canonicalize_capability_names(
        crate::capabilities::RuntimeFacet::Compute,
        cfg_list(
            crate::capabilities::DISABLED_PROVIDERS_CONFIG.env,
            crate::capabilities::DISABLED_PROVIDERS_CONFIG.path,
            &[],
        ),
    )
});
static WC_PROVIDERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut providers = canonicalize_capability_names(
        crate::capabilities::RuntimeFacet::Compute,
        cfg_list(
            crate::capabilities::PROVIDERS_CONFIG.env,
            crate::capabilities::PROVIDERS_CONFIG.path,
            DEFAULT_PROVIDERS,
        ),
    );
    providers.retain(|provider| !WC_DISABLED_PROVIDERS.contains(provider));
    providers
});

/// Multi-provider dispatch (env `WC_PROVIDERS`, comma-separated).
/// Coordinator and Cloud Function ticks iterate this list, calling
/// check_running_jobs / reap_dead_agents / schedule_queued_jobs per
/// provider. A provider whose constructor throws (creds missing) is logged
/// and skipped. An unconfigured deployment has no provider rather than a
/// hidden GCP dependency.
pub fn wc_providers() -> &'static [String] {
    &WC_PROVIDERS
}

/// Explicitly disabled entries from the configured provider preference order.
/// Keeping this separate from [`wc_providers`] lets deployment config explain
/// why a provisioned provider is fenced without letting the scheduler call it.
pub fn wc_disabled_providers() -> &'static [String] {
    &WC_DISABLED_PROVIDERS
}

static WC_STORAGE_BACKEND: LazyLock<String> = LazyLock::new(|| resolve_storage_backend(false));
static WC_AZURE_STORAGE_ACCOUNT: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "account",
        false,
        "",
    )
});
/// An Azure deployment must name its container explicitly. Exported so
/// doctor/deploy preflight can distinguish configured state from the
/// provider-neutral empty default.
pub const DEFAULT_AZURE_CONTAINER: &str = "";
static WC_AZURE_CONTAINER: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "container",
        false,
        DEFAULT_AZURE_CONTAINER,
    )
});
static WC_S3_BUCKET: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::S3, "bucket", false, "")
});
static WC_S3_REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::S3,
        "region",
        false,
        "us-east-1",
    )
});
static WC_STADO_STORAGE_URL: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "url",
        false,
        "",
    )
});
static WC_STADO_STORAGE_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "token-file",
        false,
        "",
    )
});
static WC_STADO_STORAGE_NAMESPACE: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "namespace",
        false,
        "",
    )
});
static WC_STADO_STORAGE_CA_FILE: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "ca-file",
        false,
        "",
    )
});
static WC_LOCAL_STORAGE_PATH: LazyLock<String> = LazyLock::new(|| {
    let default = expand_tilde("~/.stado/local-storage");
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::Local,
        "path",
        false,
        &default.to_string_lossy(),
    )
});
static WC_BACKUP_STORAGE_BACKEND: LazyLock<String> =
    LazyLock::new(|| resolve_storage_backend(true));
static WC_BACKUP_BUCKET: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::S3, "bucket", true, "")
});
static WC_BACKUP_AZURE_STORAGE_ACCOUNT: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "account",
        true,
        "",
    )
});
static WC_BACKUP_AZURE_CONTAINER: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "container",
        true,
        "",
    )
});
static WC_BACKUP_S3_REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::S3, "region", true, "")
});
static WC_BACKUP_LOCAL_STORAGE_PATH: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::Local, "path", true, "")
});

/// Queue storage backend (env `WC_STORAGE_BACKEND`). "gcs", "azure", and
/// "s3" support shared workers; "local" is a device-local deployment
/// rooted at [`wc_local_storage_path`].
pub fn wc_storage_backend() -> &'static str {
    WC_STORAGE_BACKEND.as_str()
}

/// Azure storage account for the queue backend (env
/// `WC_AZURE_STORAGE_ACCOUNT`).
pub fn wc_azure_storage_account() -> &'static str {
    WC_AZURE_STORAGE_ACCOUNT.as_str()
}

/// Azure blob container for the queue backend (env `WC_AZURE_CONTAINER`).
pub fn wc_azure_container() -> &'static str {
    WC_AZURE_CONTAINER.as_str()
}

/// S3 bucket for the queue backend (env `WC_S3_BUCKET`).
pub fn wc_s3_bucket() -> &'static str {
    WC_S3_BUCKET.as_str()
}

/// S3 region (env `WC_S3_REGION`, falling back to `AWS_REGION`, then
/// us-east-1).
pub fn wc_s3_region() -> &'static str {
    WC_S3_REGION.as_str()
}

/// HTTPS origin of the Stado object API used as shared queue storage.
pub fn wc_stado_storage_url() -> &'static str {
    WC_STADO_STORAGE_URL.as_str()
}

/// Owner-only file containing the scoped Stado object API bearer token.
pub fn wc_stado_storage_token_file() -> &'static str {
    WC_STADO_STORAGE_TOKEN_FILE.as_str()
}

/// Object namespace containing this deployment's complete queue state.
pub fn wc_stado_storage_namespace() -> &'static str {
    WC_STADO_STORAGE_NAMESPACE.as_str()
}

/// PEM root certificate that signs the Stado object API's HTTPS endpoint.
///
/// A fleet that publishes its object API on the tailnet is served by a private
/// certificate authority the operating system has never heard of. Without this the
/// client has only the system roots, every request to that endpoint dies in the
/// handshake as "error sending request", and the sole configuration left standing
/// is a loopback URL -- so each host addresses its own store and the fleet stops
/// sharing one registry. Empty means a publicly trusted authority, or loopback.
pub fn wc_stado_storage_ca_file() -> &'static str {
    WC_STADO_STORAGE_CA_FILE.as_str()
}

/// Root directory of the device-local storage backend (env
/// `WC_LOCAL_STORAGE_PATH`).
pub fn wc_local_storage_path() -> &'static str {
    WC_LOCAL_STORAGE_PATH.as_str()
}

/// Disaster-recovery storage backend. Empty means no backup is configured.
///
/// Queue mutations commit to the configured primary and are then mirrored
/// best-effort to this endpoint. Reads consult it only when the primary
/// returns an error; an authoritative primary `absent` result never falls
/// through, so the backup cannot become a second writer or dispatch queue.
pub fn wc_backup_storage_backend() -> &'static str {
    WC_BACKUP_STORAGE_BACKEND.as_str()
}

/// GCS or S3 bucket used by the disaster-recovery endpoint.
pub fn wc_backup_bucket() -> &'static str {
    WC_BACKUP_BUCKET.as_str()
}

/// Azure account used by the disaster-recovery endpoint.
pub fn wc_backup_azure_storage_account() -> &'static str {
    WC_BACKUP_AZURE_STORAGE_ACCOUNT.as_str()
}

/// Azure container used by the disaster-recovery endpoint.
pub fn wc_backup_azure_container() -> &'static str {
    WC_BACKUP_AZURE_CONTAINER.as_str()
}

/// S3 region used by the disaster-recovery endpoint.
pub fn wc_backup_s3_region() -> &'static str {
    WC_BACKUP_S3_REGION.as_str()
}

/// Local path used by the disaster-recovery endpoint.
pub fn wc_backup_local_storage_path() -> &'static str {
    WC_BACKUP_LOCAL_STORAGE_PATH.as_str()
}

/// Canonical Stado API origin used by object and immutable-release clients.
/// `api.url` is the deployment endpoint; releases do not own a second origin.
pub fn stado_api_url() -> String {
    cfg("STADO_API_URL", "api.url", "")
        .trim_end_matches('/')
        .to_string()
}

/// Public origin that serves the three enrollment routes (`GET /join.sh`,
/// `GET /api/fleet/invite/key`, `POST /api/fleet/join`) — env
/// `STADO_ENROLLMENT_URL`, config key `enrollment.url`, empty by default.
///
/// This is deliberately NOT [`stado_api_url`]. `api.url` is the deployment
/// endpoint that self-update, remote bootstrap, cloud-agent dispatch and the
/// coordinator resolve their release channel from; pointing it at a narrow
/// enrollment listener would break all of those. A publicly tunnelled
/// `stado dashboard --enrollment-only` listener serves only enrollment, so it
/// needs its own origin. Empty means "no separate enrollment origin", and
/// every caller falls back to `api.url` exactly as before.
pub fn enrollment_url() -> String {
    cfg("STADO_ENROLLMENT_URL", "enrollment.url", "")
        .trim_end_matches('/')
        .to_string()
}

/// Exact immutable Stado version consumed by bootstrap and cloud agents (env
/// `STADO_RELEASE_VERSION`, config key `release.version`).
pub fn stado_release_version() -> String {
    cfg("STADO_RELEASE_VERSION", "release.version", "")
        .trim()
        .to_string()
}

/// Exact release platform shipped to cloud-agent templates (env
/// `STADO_RELEASE_PLATFORM`, config key `release.platform`). Remote bootstrap
/// derives its exact platform from the remote kernel and architecture.
pub fn stado_release_platform() -> String {
    cfg("STADO_RELEASE_PLATFORM", "release.platform", "")
        .trim()
        .to_string()
}

/// Skarbiec key-pair item containing the base64 Ed25519 PKCS#8 release
/// authority key in `private_key`. The item name is configuration; key bytes
/// never enter a product manifest or registry document.
pub fn release_signing_key_item() -> String {
    cfg(
        "STADO_RELEASE_SIGNING_KEY_ITEM",
        "release.signing_key_item",
        "stado-release-signing",
    )
    .trim()
    .to_string()
}

/// Trusted release-control key identifier paired with
/// [`release_signing_key_item`].
pub fn release_signing_key_id() -> String {
    cfg(
        "STADO_RELEASE_SIGNING_KEY_ID",
        "release.signing_key_id",
        "stado-release-2026-08",
    )
    .trim()
    .to_string()
}

/// Exact immutable release object containing the cloud-agent Python
/// environment and model cache. There is deliberately no default: dispatch
/// refuses to create a machine until the operator publishes and selects one.
pub fn stado_agent_runtime_bundle_uri() -> String {
    cfg(
        "STADO_AGENT_RUNTIME_BUNDLE_URI",
        "release.agent_runtime_bundle_uri",
        "",
    )
    .trim()
    .to_string()
}

/// SHA-256 of [`stado_agent_runtime_bundle_uri`], checked before extraction.
pub fn stado_agent_runtime_bundle_sha256() -> String {
    cfg(
        "STADO_AGENT_RUNTIME_BUNDLE_SHA256",
        "release.agent_runtime_bundle_sha256",
        "",
    )
    .trim()
    .to_string()
}

static BILLING_PROVIDERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    canonicalize_capability_names(
        crate::capabilities::RuntimeFacet::Billing,
        cfg_list(
            "WC_BILLING_PROVIDERS",
            "billing.providers",
            &["gcp", "azure"],
        ),
    )
});
static BILLING_DATASET: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_BILLING_DATASET").unwrap_or_else(|_| "billing_export".to_string())
});
static BILLING_TABLE: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_BILLING_TABLE")
        .unwrap_or_else(|_| "gcp_billing_export_v1_017364_D3B657_F207B5".to_string())
});
static BILLING_NET_ALERT_USD: LazyLock<f64> = LazyLock::new(|| {
    std::env::var("WC_BILLING_NET_ALERT_USD")
        .unwrap_or_else(|_| "100".to_string())
        .parse::<f64>()
        .expect("WC_BILLING_NET_ALERT_USD must be a number")
});
static AZURE_BILLING_SECRET: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_AZURE_BILLING_SECRET")
        .unwrap_or_else(|_| "wisent-azure-billing-sp".to_string())
});
static AZURE_PROVIDER_SECRET: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_AZURE_SECRET").unwrap_or_else(|_| "stado-azure".to_string())
});
static SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SKARBIEC_URL",
        "secrets.skarbiec.url",
        "http://127.0.0.1:17602",
    )
});
static SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SKARBIEC_CONSUMER",
        "secrets.skarbiec.consumer",
        "stado-control-plane",
    )
});
static SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("control-plane-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_SKARBIEC_TOKEN_FILE",
        "secrets.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});
static AGENT_SKARBIEC_URL: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AGENT_SKARBIEC_URL", "agent.skarbiec.url", ""));
static AGENT_SKARBIEC_CONSUMER: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AGENT_SKARBIEC_CONSUMER", "agent.skarbiec.consumer", ""));
static AGENT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("workload-agent-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_AGENT_SKARBIEC_TOKEN_FILE",
        "agent.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});
static AGENT_SKARBIEC_ITEMS: LazyLock<Vec<String>> =
    LazyLock::new(|| cfg_list("WC_AGENT_SKARBIEC_ITEMS", "agent.skarbiec.items", &[]));
static AGENT_SKARBIEC_SECRET_FIELDS: LazyLock<Vec<String>> = LazyLock::new(|| {
    cfg_list(
        "WC_AGENT_SKARBIEC_SECRET_FIELDS",
        "agent.skarbiec.secret_fields",
        &[],
    )
});
static BACKEND_MESSAGING_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_BACKEND_MESSAGING_SKARBIEC_URL",
        "backend.messaging.skarbiec.url",
        skarbiec_url(),
    )
});
static BACKEND_MESSAGING_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_BACKEND_MESSAGING_SKARBIEC_CONSUMER",
        "backend.messaging.skarbiec.consumer",
        "",
    )
});
static BACKEND_MESSAGING_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    expand_tilde(&cfg(
        "WC_BACKEND_MESSAGING_SKARBIEC_TOKEN_FILE",
        "backend.messaging.skarbiec.token_file",
        "",
    ))
    .to_string_lossy()
    .into_owned()
});
static BACKEND_MESSAGING_SKARBIEC_ITEMS: LazyLock<Vec<String>> = LazyLock::new(|| {
    cfg_list(
        "WC_BACKEND_MESSAGING_SKARBIEC_ITEMS",
        "backend.messaging.skarbiec.items",
        &[],
    )
});

/// Product namespaces that must have explicit object-gateway credentials.
/// `releases` is intentionally absent: it remains on the dedicated public
/// GET-only release route.
pub const ACTIVE_OBJECT_NAMESPACES: &[&str] = &[
    "entitlements-rotator",
    "echo",
    "content-platform",
    "growth-tactics",
    "needher",
    "oko",
    "openenv",
    "probierz",
    "trading-autonomy",
    "trading-tools",
    "weles",
    "wisent-app",
    "wisent-backend",
    "wisent-images",
    "wisent-tools",
    "wisent-trade",
];

pub const OBJECT_API_VERIFIER_CONSUMER: &str = "stado-object-api-verifier";
/// Route-scoped bearer the dashboard verifies for host-health publication.
///
/// The object verifier reads this item too because the host-health endpoint is
/// served by the same dashboard process and must not fall back to the broad
/// coordinator grant.
pub const HOST_HEALTH_API_ITEM: &str = "stado-host-health-api";

pub const OBJECT_API_ACTIONS: &[&str] = &["delete", "get", "list", "put", "stat"];

/// One exact object-key boundary and the actions granted inside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectPrefixPolicy {
    prefix: String,
    actions: Vec<String>,
}

impl ObjectPrefixPolicy {
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn actions(&self) -> &[String] {
        &self.actions
    }

    fn allows_action(&self, action: &str) -> bool {
        self.actions.iter().any(|allowed| allowed == action)
    }

    fn contains_key(&self, key: &str) -> bool {
        if self.prefix.is_empty() {
            true
        } else if self.prefix.ends_with('/') {
            key.starts_with(&self.prefix)
        } else {
            key == self.prefix
        }
    }
}

/// One product credential and its least-privilege key/action boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectApiNamespace {
    item: String,
    prefix_policies: Vec<ObjectPrefixPolicy>,
}

impl ObjectApiNamespace {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn prefix_policies(&self) -> &[ObjectPrefixPolicy] {
        &self.prefix_policies
    }

    /// Whether one canonical object key and action are granted together.
    pub fn allows_object_action(&self, key: &str, action: &str) -> bool {
        self.prefix_policies
            .iter()
            .any(|policy| policy.allows_action(action) && policy.contains_key(key))
    }

    /// Authorize and canonicalize a list prefix under one policy that grants
    /// list. Exact-key policies can never authorize a prefix scan.
    ///
    /// The caller's trailing `/` survives: authorization compares paths, and
    /// the string this returns is what the store will scan. A policy with an
    /// empty prefix grants the namespace, and returning a trimmed `queue` for
    /// a requested `queue/` is what let a listing reach `queue_priority/`.
    pub fn authorized_list_prefix(&self, requested: &str, action: &str) -> Option<String> {
        let requested = requested.trim_start_matches('/');
        let path = requested.trim_end_matches('/');
        self.prefix_policies.iter().find_map(|policy| {
            if !policy.allows_action(action) {
                return None;
            }
            let allowed = policy.prefix();
            if allowed.is_empty() {
                return Some(requested.to_string());
            }
            if !allowed.ends_with('/') {
                return None;
            }
            let root = allowed.strip_suffix('/').unwrap_or(allowed);
            if path == root {
                Some(allowed.to_string())
            } else if path.starts_with(allowed) {
                Some(requested.to_string())
            } else {
                None
            }
        })
    }
}

fn valid_object_prefix(prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if prefix.trim() != prefix || prefix.starts_with('/') {
        return false;
    }
    let is_subtree = prefix.ends_with('/');
    let path = prefix.trim_end_matches('/');
    !path.is_empty()
        && (is_subtree || !path.contains('/'))
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        && !path.contains('\0')
        && !path.contains('\\')
}

fn object_prefixes_overlap(left: &str, right: &str) -> bool {
    left == right
        || left.is_empty()
        || right.is_empty()
        || (left.ends_with('/') && right.starts_with(left))
        || (right.ends_with('/') && left.starts_with(right))
}

fn parse_object_actions(
    value: Option<&Value>,
    location: &str,
    use_default: bool,
    problems: &mut Vec<String>,
) -> Vec<String> {
    let Some(value) = value else {
        if use_default {
            return OBJECT_API_ACTIONS
                .iter()
                .map(|action| (*action).to_string())
                .collect();
        }
        problems.push(format!("{location} is required"));
        return Vec::new();
    };
    let Value::Array(values) = value else {
        problems.push(format!("{location} must be an array of actions"));
        return Vec::new();
    };
    if values.is_empty() {
        problems.push(format!("{location} must not be empty"));
        return Vec::new();
    }
    let mut parsed = Vec::with_capacity(values.len());
    let mut seen = BTreeSet::new();
    for value in values {
        let Some(action) = value.as_str() else {
            problems.push(format!("{location} entries must be strings"));
            continue;
        };
        if !OBJECT_API_ACTIONS.contains(&action) {
            problems.push(format!("{location} contains unsupported action {action:?}"));
            continue;
        }
        if !seen.insert(action) {
            problems.push(format!("{location} contains duplicate action {action:?}"));
            continue;
        }
        parsed.push(action.to_string());
    }
    parsed
}

fn parse_legacy_object_prefixes(
    value: Option<&Value>,
    location: &str,
    problems: &mut Vec<String>,
) -> Vec<String> {
    let Some(value) = value else {
        return vec![String::new()];
    };
    let Value::Array(values) = value else {
        problems.push(format!("{location} must be an array of strings"));
        return Vec::new();
    };
    if values.is_empty() {
        problems.push(format!("{location} must not be empty"));
        return Vec::new();
    }
    let mut parsed: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let Some(prefix) = value.as_str() else {
            problems.push(format!("{location} entries must be strings"));
            continue;
        };
        if !valid_object_prefix(prefix) {
            problems.push(format!(
                "{location} entry {prefix:?} must be empty for namespace root, a canonical top-level object key, or a canonical path ending in '/'"
            ));
            continue;
        }
        if let Some(earlier) = parsed
            .iter()
            .find(|earlier| object_prefixes_overlap(earlier, prefix))
        {
            problems.push(format!(
                "{location} contains ambiguous overlapping prefixes {earlier:?} and {prefix:?}"
            ));
            continue;
        }
        parsed.push(prefix.to_string());
    }
    parsed
}

/// Parse the security-sensitive namespace map without applying defaults.
/// Each item name is bound to its namespace by construction, preventing a
/// typo from granting product A the bearer belonging to product B.
pub(crate) fn parse_object_api_namespaces(
    value: Option<&Value>,
) -> Result<BTreeMap<String, ObjectApiNamespace>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "object_api.namespaces must be a non-empty object mapping namespaces to Skarbiec items"
                .to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec![
            "object_api.namespaces must not be empty; product object routes fail closed without an explicit mapping"
                .to_string(),
        ]);
    }

    let mut problems = Vec::new();
    let mut namespaces = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (namespace, raw_entry) in entries {
        let problem_count = problems.len();
        if namespace.trim() != namespace
            || namespace == "releases"
            || crate::object_store::ObjectRef::new(namespace, "sentinel").is_err()
        {
            problems.push(format!(
                "object_api.namespaces key {namespace:?} is not a canonical private product namespace"
            ));
        }
        let Some(entry) = raw_entry.as_object() else {
            problems.push(format!(
                "object_api.namespaces.{namespace} must be an object with item and either prefix_policies or legacy prefixes/actions"
            ));
            continue;
        };
        for key in entry.keys() {
            if !matches!(
                key.as_str(),
                "item" | "prefixes" | "actions" | "prefix_policies"
            ) {
                problems.push(format!(
                    "object_api.namespaces.{namespace} contains unsupported key {key:?}"
                ));
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_item = if namespace == "wisent-backend" {
            "wisent-backend-object-client".to_string()
        } else {
            format!("{namespace}-object-api")
        };
        if item != expected_item {
            problems.push(format!(
                "object_api.namespaces.{namespace}.item must be {expected_item:?}, got {item:?}"
            ));
        }

        let mut prefix_policies = Vec::new();
        if let Some(explicit) = entry.get("prefix_policies") {
            if entry.contains_key("prefixes") || entry.contains_key("actions") {
                problems.push(format!(
                    "object_api.namespaces.{namespace} cannot combine prefix_policies with legacy prefixes/actions"
                ));
            }
            match explicit {
                Value::Array(values) if !values.is_empty() => {
                    for (index, raw_policy) in values.iter().enumerate() {
                        let location =
                            format!("object_api.namespaces.{namespace}.prefix_policies[{index}]");
                        let Some(policy) = raw_policy.as_object() else {
                            problems.push(format!(
                                "{location} must be an object with exact prefix and actions"
                            ));
                            continue;
                        };
                        for key in policy.keys() {
                            if !matches!(key.as_str(), "prefix" | "actions") {
                                problems
                                    .push(format!("{location} contains unsupported key {key:?}"));
                            }
                        }
                        let prefix = policy
                            .get("prefix")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if prefix.is_empty() || !valid_object_prefix(prefix) {
                            problems.push(format!(
                                "{location}.prefix must be a non-empty canonical top-level object key or path ending in '/'"
                            ));
                        }
                        let actions = parse_object_actions(
                            policy.get("actions"),
                            &format!("{location}.actions"),
                            false,
                            &mut problems,
                        );
                        if let Some(earlier) =
                            prefix_policies
                                .iter()
                                .find(|earlier: &&ObjectPrefixPolicy| {
                                    object_prefixes_overlap(earlier.prefix(), prefix)
                                })
                        {
                            problems.push(format!(
                                "{location}.prefix {prefix:?} ambiguously overlaps earlier prefix {:?}",
                                earlier.prefix()
                            ));
                        }
                        prefix_policies.push(ObjectPrefixPolicy {
                            prefix: prefix.to_string(),
                            actions,
                        });
                    }
                }
                Value::Array(_) => problems.push(format!(
                    "object_api.namespaces.{namespace}.prefix_policies must not be empty"
                )),
                _ => problems.push(format!(
                    "object_api.namespaces.{namespace}.prefix_policies must be an array"
                )),
            }
        } else {
            let prefixes = parse_legacy_object_prefixes(
                entry.get("prefixes"),
                &format!("object_api.namespaces.{namespace}.prefixes"),
                &mut problems,
            );
            let actions = parse_object_actions(
                entry.get("actions"),
                &format!("object_api.namespaces.{namespace}.actions"),
                true,
                &mut problems,
            );
            prefix_policies.extend(prefixes.into_iter().map(|prefix| ObjectPrefixPolicy {
                prefix,
                actions: actions.clone(),
            }));
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "object_api.namespaces maps more than one namespace to item {item:?}"
            ));
        }
        if problems.len() == problem_count {
            namespaces.insert(
                namespace.to_string(),
                ObjectApiNamespace {
                    item: item.to_string(),
                    prefix_policies,
                },
            );
        }
    }
    for &required in ACTIVE_OBJECT_NAMESPACES {
        if !namespaces.contains_key(required) {
            problems.push(format!(
                "object_api.namespaces is missing active namespace {required:?}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(namespaces)
    } else {
        Err(problems)
    }
}

/// The namespace the queue lives in; every prefix in
/// [`crate::queue::copy::CANONICAL_PREFIXES`] is read and written there.
pub const QUEUE_OBJECT_NAMESPACE: &str = "probierz";

/// The five actions the queue performs on its own prefixes.
const QUEUE_OBJECT_ACTIONS: [&str; 5] = ["get", "put", "list", "stat", "delete"];

/// Canonical queue prefixes the `probierz` object policy does not grant for
/// every queue action, sorted; empty when the policy covers the queue.
///
/// A binary that reads a prefix nobody granted does not fail at the line
/// that reads it: the object API answers 401, the agent logs "agent loop
/// failed" and restarts, and the host claims nothing while its capacity
/// broadcast keeps saying it is alive. The declaration lives in each object
/// API host's config and the consumer lives in the binary, so the check
/// belongs where the two meet: `stado config validate` and `config set` on
/// the host, and `stado doctor` there.
pub fn queue_prefixes_missing(
    namespaces: &BTreeMap<String, ObjectApiNamespace>,
) -> Vec<&'static str> {
    let Some(policy) = namespaces.get(QUEUE_OBJECT_NAMESPACE) else {
        return Vec::new();
    };
    let mut missing: Vec<&'static str> = crate::queue::copy::CANONICAL_PREFIXES
        .iter()
        .copied()
        .filter(|prefix| {
            // A prefix is probed with a key under it; a root object is
            // probed as itself.
            let key = if prefix.ends_with('/') {
                format!("{prefix}probe")
            } else {
                (*prefix).to_string()
            };
            !QUEUE_OBJECT_ACTIONS
                .iter()
                .all(|action| policy.allows_object_action(&key, action))
        })
        .collect();
    missing.sort_unstable();
    missing
}

/// One sentence for a policy that leaves the queue's prefixes ungranted,
/// naming them and the command that grants them; `None` when it covers them.
pub fn queue_prefix_problem(namespaces: &BTreeMap<String, ObjectApiNamespace>) -> Option<String> {
    let missing = queue_prefixes_missing(namespaces);
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "object_api.namespaces.{QUEUE_OBJECT_NAMESPACE} does not grant the queue prefix(es) {} for \
         get, put, list, stat and delete; every agent claim against this object API answers 401. \
         Add each as a prefix_policies entry: stado host config-set <target> \
         object_api.namespaces.{QUEUE_OBJECT_NAMESPACE} '<json>' --reload-service <object-api unit>",
        missing.join(", ")
    ))
}

static OBJECT_API_NAMESPACES: LazyLock<Result<BTreeMap<String, ObjectApiNamespace>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_OBJECT_API_NAMESPACES")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_OBJECT_API_NAMESPACES must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("object_api.namespaces"),
        };
        parse_object_api_namespaces(configured.as_ref())
    });

/// The least-privilege consumer that may read the alert credential.
///
/// Paging is the last thing that should hold a broad grant, and the fleet
/// already provisions a consumer carrying exactly one read on the resend key.
pub const ALERT_KEY_READER_CONSUMER: &str = "weles-resend-management-client";
static ALERT_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_ALERT_SKARBIEC_CONSUMER",
        "alerts.skarbiec.consumer",
        ALERT_KEY_READER_CONSUMER,
    )
});
static ALERT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("weles-resend-management-client-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_ALERT_SKARBIEC_TOKEN_FILE",
        "alerts.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub fn alert_skarbiec_consumer() -> &'static str {
    &ALERT_SKARBIEC_CONSUMER
}

pub fn alert_skarbiec_token_file() -> &'static str {
    &ALERT_SKARBIEC_TOKEN_FILE
}
static OBJECT_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_OBJECT_SKARBIEC_URL",
        "object_api.skarbiec.url",
        skarbiec_url(),
    )
});
static OBJECT_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_OBJECT_SKARBIEC_CONSUMER",
        "object_api.skarbiec.consumer",
        OBJECT_API_VERIFIER_CONSUMER,
    )
});
static OBJECT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-object-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_OBJECT_SKARBIEC_TOKEN_FILE",
        "object_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

/// Active authenticated software publishers. Public readers use the separate
/// tokenless release GET route.
pub const ACTIVE_RELEASE_PUBLISHERS: &[&str] = &[
    "brama",
    "compute-marketplace",
    "image-video-router",
    "oko",
    "skarbiec",
    "stado",
    "trading-autonomy",
    "wisent-backend",
];

pub const RELEASE_API_VERIFIER_CONSUMER: &str = "stado-release-api-verifier";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePublisher {
    item: String,
    prefix: String,
}

impl ReleasePublisher {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn allows_key(&self, key: &str) -> bool {
        key.starts_with(&self.prefix)
    }

    /// The authorized release listing prefix, with the caller's trailing `/`
    /// intact for the reason [`ObjectApiNamespace::authorized_list_prefix`]
    /// gives: the separator decides whether a scan stays inside a coordinate
    /// or reaches every sibling whose name begins with it.
    pub fn authorized_list_prefix(&self, requested: &str) -> Option<String> {
        let requested = requested.trim_start_matches('/');
        let path = requested.trim_end_matches('/');
        let root = self.prefix.strip_suffix('/').unwrap_or(&self.prefix);
        if path == root {
            Some(self.prefix.clone())
        } else if path.starts_with(&self.prefix) {
            Some(requested.to_string())
        } else {
            None
        }
    }
}

pub(crate) fn parse_release_publishers(
    value: Option<&Value>,
) -> Result<BTreeMap<String, ReleasePublisher>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "release_api.publishers must be a non-empty product-to-item mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec![
            "release_api.publishers must not be empty; authenticated release writes fail closed"
                .to_string(),
        ]);
    }

    let mut problems = Vec::new();
    let mut publishers = BTreeMap::new();
    let mut items = BTreeSet::new();
    let mut prefixes = BTreeSet::new();
    for (product, raw_entry) in entries {
        let mut entry_valid = true;
        if product.trim() != product
            || crate::object_store::ObjectRef::new(product, "sentinel").is_err()
        {
            problems.push(format!(
                "release_api.publishers key {product:?} is not a canonical product name"
            ));
            entry_valid = false;
        }
        let Some(entry) = raw_entry.as_object() else {
            problems.push(format!(
                "release_api.publishers.{product} must be an object with item and prefix"
            ));
            continue;
        };
        for key in entry.keys() {
            if key != "item" && key != "prefix" {
                problems.push(format!(
                    "release_api.publishers.{product} contains unsupported key {key:?}"
                ));
                entry_valid = false;
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_item = format!("{product}-release-publisher");
        if item != expected_item {
            problems.push(format!(
                "release_api.publishers.{product}.item must be {expected_item:?}, got {item:?}"
            ));
            entry_valid = false;
        }
        let prefix = entry
            .get("prefix")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_prefix = format!("{product}/");
        if prefix != expected_prefix {
            problems.push(format!(
                "release_api.publishers.{product}.prefix must be {expected_prefix:?}, got {prefix:?}"
            ));
            entry_valid = false;
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "release_api.publishers maps more than one product to item {item:?}"
            ));
            entry_valid = false;
        }
        if !prefixes.insert(prefix.to_string()) {
            problems.push(format!(
                "release_api.publishers maps more than one product to prefix {prefix:?}"
            ));
            entry_valid = false;
        }
        if entry_valid {
            publishers.insert(
                product.to_string(),
                ReleasePublisher {
                    item: item.to_string(),
                    prefix: prefix.to_string(),
                },
            );
        }
    }
    for &required in ACTIVE_RELEASE_PUBLISHERS {
        if !publishers.contains_key(required) {
            problems.push(format!(
                "release_api.publishers is missing active publisher {required:?}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(publishers)
    } else {
        Err(problems)
    }
}

static RELEASE_API_PUBLISHERS: LazyLock<Result<BTreeMap<String, ReleasePublisher>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_RELEASE_API_PUBLISHERS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_RELEASE_API_PUBLISHERS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("release_api.publishers"),
        };
        parse_release_publishers(configured.as_ref())
    });
static RELEASE_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RELEASE_SKARBIEC_URL",
        "release_api.skarbiec.url",
        skarbiec_url(),
    )
});
static RELEASE_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RELEASE_SKARBIEC_CONSUMER",
        "release_api.skarbiec.consumer",
        RELEASE_API_VERIFIER_CONSUMER,
    )
});
static RELEASE_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-release-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_RELEASE_SKARBIEC_TOKEN_FILE",
        "release_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

/// The consumer the vault already authorizes to read the release authority's
/// private key, and nothing else: its single minted capability is
/// `read:stado-release-signing#private_key`.
///
/// `release submit` read that key through `secrets.skarbiec.consumer`, the broad
/// control-plane grant, which the vault correctly refuses. The refusal arrived as
/// a bare `403 consumer not authorized to read item field` naming neither the
/// consumer it wanted nor the one it got, and the vault's own policy had the
/// answer the whole time.
pub const RELEASE_SIGNING_CONSUMER: &str = "stado-release-coordinator";

static RELEASE_SIGNING_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RELEASE_SIGNING_SKARBIEC_CONSUMER",
        "release.signing_skarbiec.consumer",
        RELEASE_SIGNING_CONSUMER,
    )
});
static RELEASE_SIGNING_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-release-coordinator-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_RELEASE_SIGNING_SKARBIEC_TOKEN_FILE",
        "release.signing_skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub const REGISTRY_API_VERIFIER_CONSUMER: &str = "stado-registry-api-verifier";
/// Actions a registry-API client may be granted.
///
/// `policy-read` and `cleanup-read` answer questions; `policy-write` rewrites
/// one target's whitelisted policy fields and `cleanup-run` asks the local
/// janitor for a pass. They are separate because reading a fleet's policy and
/// rewriting it are not the same authority, and the desktop app asks for them
/// with separate requests.
pub const REGISTRY_API_ACTIONS: &[&str] =
    &["cleanup-read", "cleanup-run", "policy-read", "policy-write"];

/// One client authorized against the registry-policy boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryApiClient {
    item: String,
    actions: Vec<String>,
}

impl RegistryApiClient {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows_action(&self, action: &str) -> bool {
        self.actions.iter().any(|allowed| allowed == action)
    }
}

/// Parse `registry_api.clients`.
///
/// An ABSENT or empty mapping is `Ok(empty)`, not an error, and that is the
/// whole judgement in this function. Every route on this boundary then
/// refuses with `401`, because nobody has been granted it — which is a
/// refusal, not an outage. Returning `Err` here would make an undeclared
/// boundary answer `503 authorization unavailable` and send an operator to
/// look for a broken vault instead of an absent declaration; this file
/// already learned that distinction where the janitor reads a policy, and it
/// is the same distinction.
///
/// A mapping that EXISTS and cannot be read is still an error: a declaration
/// nobody can parse is a configuration fault, and 503 is the honest answer.
pub(crate) fn parse_registry_api_clients(
    value: Option<&Value>,
) -> Result<BTreeMap<String, RegistryApiClient>, Vec<String>> {
    let mut clients = BTreeMap::new();
    let entries = match value {
        None => return Ok(clients),
        Some(Value::Object(entries)) if entries.is_empty() => return Ok(clients),
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            return Err(vec![
                "registry_api.clients must be an exact client mapping".to_string()
            ])
        }
    };
    let mut problems = Vec::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let Some(entry) = raw.as_object() else {
            problems.push(format!("registry_api.clients.{name} must be an object"));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "item" | "actions") {
                problems.push(format!(
                    "registry_api.clients.{name} contains unsupported key {key:?}"
                ));
            }
        }
        // The item name is derived, not chosen, so a client cannot be pointed
        // at another boundary's credential by editing one string.
        let expected_item = format!("{name}-registry-api");
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if item != expected_item {
            problems.push(format!(
                "registry_api.clients.{name}.item must be {expected_item:?}"
            ));
        }
        if !items.insert(item.clone()) {
            problems.push(format!(
                "registry_api.clients maps more than one client to item {item:?}"
            ));
        }
        let mut actions = Vec::new();
        match entry.get("actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(action) = value.as_str() else {
                        problems.push(format!(
                            "registry_api.clients.{name}.actions entries must be strings"
                        ));
                        continue;
                    };
                    if !REGISTRY_API_ACTIONS.contains(&action) {
                        problems.push(format!(
                            "registry_api.clients.{name}.actions contains unknown action {action:?}"
                        ));
                        continue;
                    }
                    if !seen.insert(action) {
                        problems.push(format!(
                            "registry_api.clients.{name}.actions repeats {action:?}"
                        ));
                        continue;
                    }
                    actions.push(action.to_string());
                }
            }
            _ => problems.push(format!(
                "registry_api.clients.{name}.actions must be a non-empty array"
            )),
        }
        actions.sort();
        clients.insert(name.clone(), RegistryApiClient { item, actions });
    }
    if problems.is_empty() {
        Ok(clients)
    } else {
        Err(problems)
    }
}

pub const MACHINE_API_VERIFIER_CONSUMER: &str = "stado-machine-api-verifier";
pub const MACHINE_API_ACTIONS: &[&str] = &["cancel", "status", "submit"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineApiClient {
    item: String,
    actions: Vec<String>,
    targets: Vec<String>,
}

impl MachineApiClient {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows_action(&self, action: &str) -> bool {
        self.actions.iter().any(|allowed| allowed == action)
    }

    pub fn allows_target(&self, target: &str) -> bool {
        self.targets.iter().any(|allowed| allowed == target)
    }

    pub fn targets(&self) -> &[String] {
        &self.targets
    }
}

fn canonical_machine_name(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn parse_machine_api_clients(
    value: Option<&Value>,
) -> Result<BTreeMap<String, MachineApiClient>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "machine_api.clients must be a non-empty exact client mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["machine_api.clients must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut clients = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_machine_name(name) {
            problems.push(format!("machine_api.clients key {name:?} is not canonical"));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!("machine_api.clients.{name} must be an object"));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "item" | "actions" | "targets") {
                problems.push(format!(
                    "machine_api.clients.{name} contains unsupported key {key:?}"
                ));
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_item = format!("{name}-machine-api");
        if item != expected_item {
            problems.push(format!(
                "machine_api.clients.{name}.item must be {expected_item:?}"
            ));
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "machine_api.clients maps more than one client to item {item:?}"
            ));
        }
        let mut actions = Vec::new();
        match entry.get("actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(action) = value.as_str() else {
                        problems.push(format!(
                            "machine_api.clients.{name}.actions entries must be strings"
                        ));
                        continue;
                    };
                    if !MACHINE_API_ACTIONS.contains(&action) || !seen.insert(action) {
                        problems.push(format!(
                            "machine_api.clients.{name}.actions contains unsupported or duplicate {action:?}"
                        ));
                        continue;
                    }
                    actions.push(action.to_string());
                }
            }
            _ => problems.push(format!(
                "machine_api.clients.{name}.actions must be a non-empty array"
            )),
        }
        let mut targets = Vec::new();
        match entry.get("targets") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(target) = value.as_str() else {
                        problems.push(format!(
                            "machine_api.clients.{name}.targets entries must be strings"
                        ));
                        continue;
                    };
                    let known = crate::capabilities::configurable_variant(
                        crate::capabilities::RuntimeFacet::Compute,
                        target,
                    )
                    .is_some();
                    if !known || !seen.insert(target) {
                        problems.push(format!(
                            "machine_api.clients.{name}.targets contains unknown or duplicate {target:?}"
                        ));
                        continue;
                    }
                    targets.push(target.to_string());
                }
            }
            _ => problems.push(format!(
                "machine_api.clients.{name}.targets must be a non-empty array"
            )),
        }
        if problems.len() == start {
            clients.insert(
                name.to_string(),
                MachineApiClient {
                    item: item.to_string(),
                    actions,
                    targets,
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(clients)
    } else {
        Err(problems)
    }
}

static REGISTRY_API_CLIENTS: LazyLock<Result<BTreeMap<String, RegistryApiClient>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_REGISTRY_API_CLIENTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_REGISTRY_API_CLIENTS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("registry_api.clients"),
        };
        parse_registry_api_clients(configured.as_ref())
    });
static REGISTRY_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_REGISTRY_SKARBIEC_URL",
        "registry_api.skarbiec.url",
        skarbiec_url(),
    )
});
static REGISTRY_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_REGISTRY_SKARBIEC_CONSUMER",
        "registry_api.skarbiec.consumer",
        REGISTRY_API_VERIFIER_CONSUMER,
    )
});
static REGISTRY_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-registry-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_REGISTRY_SKARBIEC_TOKEN_FILE",
        "registry_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

static MACHINE_API_CLIENTS: LazyLock<Result<BTreeMap<String, MachineApiClient>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_MACHINE_API_CLIENTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_MACHINE_API_CLIENTS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("machine_api.clients"),
        };
        parse_machine_api_clients(configured.as_ref())
    });
static MACHINE_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_MACHINE_SKARBIEC_URL",
        "machine_api.skarbiec.url",
        skarbiec_url(),
    )
});
static MACHINE_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_MACHINE_SKARBIEC_CONSUMER",
        "machine_api.skarbiec.consumer",
        MACHINE_API_VERIFIER_CONSUMER,
    )
});
static MACHINE_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-machine-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_MACHINE_SKARBIEC_TOKEN_FILE",
        "machine_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub const DATABASE_API_ENGINES: &[&str] = &["postgres", "sqlite"];
pub const DATABASE_API_SCOPES: &[&str] = &["read", "write"];

/// One declared fleet database: where its credential lives, what engine
/// speaks it, which scopes exist, and which consumers may resolve it.
///
/// The plane deliberately holds the coordinate, not the secret. A consumer
/// that resolves a database learns the endpoint and the Skarbiec item to
/// acquire; the value behind the item never passes through this surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseApiDatabase {
    item: String,
    engine: String,
    scopes: Vec<String>,
    consumers: Vec<String>,
}

impl DatabaseApiDatabase {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn engine(&self) -> &str {
        &self.engine
    }

    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    pub fn consumers(&self) -> &[String] {
        &self.consumers
    }

    pub fn allows_consumer(&self, consumer: &str) -> bool {
        self.consumers.iter().any(|allowed| allowed == consumer)
    }
}

pub(crate) fn parse_database_api_databases(
    value: Option<&Value>,
) -> Result<BTreeMap<String, DatabaseApiDatabase>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "database_api.databases must be a non-empty object mapping database names to declarations"
                .to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["database_api.databases must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut databases = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_machine_name(name) {
            problems.push(format!(
                "database_api.databases key {name:?} is not canonical"
            ));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!(
                "database_api.databases.{name} must be an object with item, engine, scopes and consumers"
            ));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "item" | "engine" | "scopes" | "consumers") {
                problems.push(format!(
                    "database_api.databases.{name} contains unsupported key {key:?}"
                ));
            }
        }
        let expected_item = format!("{name}-database");
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or(&expected_item);
        if item != expected_item {
            problems.push(format!(
                "database_api.databases.{name}.item must be {expected_item:?}, got {item:?}"
            ));
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "database_api.databases maps more than one database to item {item:?}"
            ));
        }
        let engine = match entry.get("engine").and_then(Value::as_str) {
            Some(engine) if DATABASE_API_ENGINES.contains(&engine) => engine.to_string(),
            Some(other) => {
                problems.push(format!(
                    "database_api.databases.{name}.engine {other:?} is not one of {DATABASE_API_ENGINES:?}"
                ));
                String::new()
            }
            None => {
                problems.push(format!("database_api.databases.{name}.engine is required"));
                String::new()
            }
        };
        let mut scopes = Vec::new();
        match entry.get("scopes") {
            Some(Value::Array(raw_scopes)) if !raw_scopes.is_empty() => {
                let mut seen = BTreeSet::new();
                for scope in raw_scopes {
                    match scope.as_str() {
                        Some(scope) if DATABASE_API_SCOPES.contains(&scope) && seen.insert(scope) => {
                            scopes.push(scope.to_string());
                        }
                        Some(other) => problems.push(format!(
                            "database_api.databases.{name}.scopes contains unsupported or duplicate {other:?}"
                        )),
                        None => problems.push(format!(
                            "database_api.databases.{name}.scopes entries must be strings"
                        )),
                    }
                }
            }
            Some(Value::Array(_)) => problems.push(format!(
                "database_api.databases.{name}.scopes must not be empty"
            )),
            Some(_) => problems.push(format!(
                "database_api.databases.{name}.scopes must be an array"
            )),
            None => problems.push(format!("database_api.databases.{name}.scopes is required")),
        }
        let mut consumers = Vec::new();
        match entry.get("consumers") {
            Some(Value::Array(raw_consumers)) if !raw_consumers.is_empty() => {
                let mut seen = BTreeSet::new();
                for consumer in raw_consumers {
                    match consumer.as_str() {
                        Some(consumer) if canonical_machine_name(consumer) && seen.insert(consumer) => {
                            consumers.push(consumer.to_string());
                        }
                        Some(other) => problems.push(format!(
                            "database_api.databases.{name}.consumers contains non-canonical or duplicate {other:?}"
                        )),
                        None => problems.push(format!(
                            "database_api.databases.{name}.consumers entries must be strings"
                        )),
                    }
                }
            }
            Some(Value::Array(_)) => problems.push(format!(
                "database_api.databases.{name}.consumers must not be empty"
            )),
            Some(_) => problems.push(format!(
                "database_api.databases.{name}.consumers must be an array"
            )),
            None => problems.push(format!(
                "database_api.databases.{name}.consumers is required"
            )),
        }
        if problems.len() == start {
            databases.insert(
                name.clone(),
                DatabaseApiDatabase {
                    item: item.to_string(),
                    engine,
                    scopes,
                    consumers,
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(databases)
    } else {
        Err(problems)
    }
}

static DATABASE_API_DATABASES: LazyLock<
    Result<BTreeMap<String, DatabaseApiDatabase>, Vec<String>>,
> = LazyLock::new(|| {
    let configured = match std::env::var("WC_DATABASE_API_DATABASES")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(encoded) => serde_json::from_str::<Value>(&encoded)
            .map_err(|error| {
                vec![format!(
                    "WC_DATABASE_API_DATABASES must be valid JSON: {error}"
                )]
            })
            .and_then(|parsed| parse_database_api_databases(Some(&parsed))),
        None => {
            parse_database_api_databases(crate::config_file::get("database_api.databases").as_ref())
        }
    };
    configured
});

pub fn database_api_databases(
) -> Result<&'static BTreeMap<String, DatabaseApiDatabase>, &'static [String]> {
    match &*DATABASE_API_DATABASES {
        Ok(databases) => Ok(databases),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub const WEB_API_EDGES: &[&str] = &["stado", "cloudflare"];

/// One declared web product: the release it runs, where it runs, the identity
/// it runs as, and the hostname it answers on.
///
/// The declaration holds no secret value. `secrets` and `database` name a
/// Skarbiec item and one of its fields; the value travels only through
/// `stado service secret-sync`, which reads it over the host channel and puts
/// it in the unit's env file without it ever reaching a command line.
///
/// A product that declares `redirect_to` is a hostname and nothing else: the
/// edge answers it with a redirect, and there is no unit, no release and no
/// host. Five of the fleet's Vercel projects are exactly that — one rewrite
/// each to `https://wisent-app.com` — and expressing them as a product with a
/// port and a consumer would mean declaring a unit nobody runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebApiProduct {
    host: String,
    port: u16,
    hostname: String,
    consumer: String,
    readyz: String,
    edge: String,
    env: BTreeMap<String, String>,
    secrets: BTreeMap<String, String>,
    database: Option<WebApiDatabase>,
    redirect_to: Option<String>,
    /// A registry service this hostname is published in front of, instead of
    /// a unit this product owns.
    ///
    /// `brama.wisent.com` is that: Brama already runs as a managed service on
    /// the mini, it is not a Node web product, and nothing about it should be
    /// built or installed by `stado web`. What was missing was a public
    /// hostname with a certificate, which is the one thing the edge does.
    upstream_service: Option<String>,
    /// A path prefix this product is mounted at, under a hostname another
    /// declaration owns.
    ///
    /// `brama.wisent.com/docs` is that: Brama's 79 documentation pages are
    /// versioned with Brama and served by a unit of their own, while the
    /// hostname's catch-all belongs to Brama itself. A mount is an ordinary
    /// unit product in every other way — built, released and deployed like
    /// any other — and only its place in the edge's configuration differs.
    path_prefix: Option<String>,
}

/// The one database a web product reads, and how its credential reaches the
/// unit: the declared database name, the field of that database's Skarbiec
/// item, and the variable the field is delivered as.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebApiDatabase {
    name: String,
    field: String,
    variable: String,
}

impl WebApiProduct {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    pub fn consumer(&self) -> &str {
        &self.consumer
    }

    pub fn readyz(&self) -> &str {
        &self.readyz
    }

    pub fn edge(&self) -> &str {
        &self.edge
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    /// Variable name to `item#field`, the same spelling
    /// `.wisent-release.json` uses for a build secret.
    pub fn secrets(&self) -> &BTreeMap<String, String> {
        &self.secrets
    }

    pub fn database(&self) -> Option<&WebApiDatabase> {
        self.database.as_ref()
    }

    /// Where this hostname redirects, for a product that is a redirect and
    /// nothing else.
    pub fn redirect_to(&self) -> Option<&str> {
        self.redirect_to.as_deref()
    }

    /// The registry service this hostname is published in front of.
    pub fn upstream_service(&self) -> Option<&str> {
        self.upstream_service.as_deref()
    }

    /// Whether this declaration describes a unit `stado web` owns.
    ///
    /// A redirect lives entirely in the edge's configuration; a hostname in
    /// front of an existing service belongs to whoever declared that service.
    /// Neither has a web release, so `deploy`, the release pipeline and the
    /// unit half of `status` have nothing to do with either.
    pub fn owns_a_unit(&self) -> bool {
        self.redirect_to.is_none() && self.upstream_service.is_none()
    }

    /// The path prefix this product is mounted at, for a product that lives
    /// under another declaration's hostname.
    pub fn path_prefix(&self) -> Option<&str> {
        self.path_prefix.as_deref()
    }

    /// Whether this product owns its hostname. A mount does not: the owner's
    /// declaration holds the record, the certificate and the catch-all.
    pub fn owns_its_hostname(&self) -> bool {
        self.path_prefix.is_none()
    }

    pub fn is_redirect(&self) -> bool {
        self.redirect_to.is_some()
    }
}

impl WebApiDatabase {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn field(&self) -> &str {
        &self.field
    }

    pub fn variable(&self) -> &str {
        &self.variable
    }
}

/// An `item#field` reference, split and checked.
///
/// The release manifest already spells a secret reference this way
/// (`release_pipeline::validate`), and a second spelling for one idea is how
/// an operator ends up with a unit whose environment nobody can trace.
pub fn parse_secret_reference(reference: &str) -> Option<(&str, &str)> {
    let (item, field) = reference.split_once('#')?;
    let identifier = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    (identifier(item) && identifier(field)).then_some((item, field))
}

/// Whether one string is a public host name: lowercase labels of letters,
/// digits and dashes, at least two of them, no trailing dot.
pub fn is_public_hostname(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.ends_with('.') {
        return false;
    }
    let labels: Vec<&str> = value.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// Whether one string can be the target of a declared redirect.
///
/// `https://` only: a redirect Stado publishes on its own edge must not send
/// a browser from a hostname it holds a certificate for to one it does not.
/// A path prefix is allowed, because a redirect to a section of a site is a
/// real thing to want; a query or a fragment is not, because the rendered
/// Caddy directive appends the incoming URI and the result would carry two
/// query strings. Braces and whitespace are refused for the reason every
/// other value written into the generated Caddyfile is: `{uri}` is the one
/// placeholder in that file, and it belongs to Stado.
pub fn is_redirect_target(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    if rest.contains('?') || rest.contains('#') {
        return false;
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
        || value.contains('{')
        || value.contains('}')
    {
        return false;
    }
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, Some(path)),
        None => (rest, None),
    };
    // A trailing slash would make the appended `{uri}` a double slash, which
    // is a different path to most servers and to every cache in between.
    if !is_public_hostname(host) || path.is_some_and(|path| path.is_empty()) {
        return false;
    }
    true
}

/// Whether one string can be the path prefix a product is mounted at.
///
/// Absolute, no trailing slash, and nothing that could change the meaning of
/// the generated `handle_path` matcher: no wildcard of its own, no brace, no
/// whitespace, no query or fragment. `/docs` mounts, `/docs/` does not,
/// because the rendered matcher is `<prefix>*` and a trailing slash would
/// stop `/docs` itself from matching.
pub fn is_mount_prefix(value: &str) -> bool {
    value.len() > 1
        && value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && !value.contains("..")
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "{}*?#".contains(c))
}

/// Whether one string can name an environment variable.
pub fn is_env_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(crate) fn parse_web_api_products(
    value: Option<&Value>,
) -> Result<BTreeMap<String, WebApiProduct>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "web_api.products must be a non-empty object mapping product names to declarations"
                .to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["web_api.products must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut products = BTreeMap::new();
    let mut hostnames = BTreeMap::new();
    let mut mounts = BTreeMap::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_machine_name(name) {
            problems.push(format!("web_api.products key {name:?} is not canonical"));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!(
                "web_api.products.{name} must be an object with host, port, hostname and consumer"
            ));
            continue;
        };
        for key in entry.keys() {
            if !matches!(
                key.as_str(),
                "host"
                    | "port"
                    | "hostname"
                    | "consumer"
                    | "readyz"
                    | "edge"
                    | "env"
                    | "secrets"
                    | "database"
                    | "redirect_to"
                    | "upstream_service"
                    | "path_prefix"
            ) {
                problems.push(format!(
                    "web_api.products.{name} contains unsupported key {key:?}"
                ));
            }
        }
        // A redirect is a hostname and a target. Every other key describes a
        // unit — where it runs, as whom, on which port, with what environment
        // — and a declaration carrying both says two different things about
        // what this product is. Refusing the combination is how the reader of
        // this section never has to guess which half won.
        let redirect_to = match entry.get("redirect_to") {
            Some(Value::String(target)) if is_redirect_target(target) => Some(target.clone()),
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.redirect_to {other} must be an https URL with a host and no query or fragment"
                ));
                None
            }
            None => None,
        };
        if redirect_to.is_some() {
            for key in [
                "host", "port", "consumer", "readyz", "env", "secrets", "database",
            ] {
                if entry.contains_key(key) {
                    problems.push(format!(
                        "web_api.products.{name} declares redirect_to and {key}: a redirect has no unit, so it has no {key}"
                    ));
                }
            }
        }
        // A hostname in front of an existing service names that service and
        // nothing about a unit: the service directory already says which host
        // it is active on and which address it answers, and repeating either
        // here would be a second copy that goes stale the day the service
        // moves.
        let upstream_service = match entry.get("upstream_service") {
            Some(Value::String(service)) if canonical_machine_name(service) => {
                Some(service.clone())
            }
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.upstream_service {other} must be a canonical registry service name"
                ));
                None
            }
            None => None,
        };
        if upstream_service.is_some() {
            for key in [
                "host", "port", "consumer", "readyz", "env", "secrets", "database",
            ] {
                if entry.contains_key(key) {
                    problems.push(format!(
                        "web_api.products.{name} declares upstream_service and {key}: the service directory answers where that service runs, so this declaration has no {key}"
                    ));
                }
            }
        }
        if upstream_service.is_some() && redirect_to.is_some() {
            problems.push(format!(
                "web_api.products.{name} declares both redirect_to and upstream_service: a hostname either answers with a redirect or forwards to a service, never both"
            ));
        }
        let host = match entry.get("host").and_then(Value::as_str) {
            Some(host) if canonical_machine_name(host) => host.to_string(),
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.host {other:?} is not a canonical target name"
                ));
                String::new()
            }
            // A redirect runs nowhere, so it names no host. The keys are
            // refused above when both are present, so this arm only ever
            // sees a declaration that is a redirect and says so.
            None if redirect_to.is_some() || upstream_service.is_some() => String::new(),
            None => {
                problems.push(format!("web_api.products.{name}.host is required"));
                String::new()
            }
        };
        // Anything below 1024 needs privilege these units deliberately do not
        // have: a web unit runs as the same login account every other managed
        // unit runs as, and the edge is what owns 443.
        let port = match entry.get("port").and_then(Value::as_u64) {
            Some(port) if (1024..=65535).contains(&port) => port as u16,
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.port {other} must be between 1024 and 65535"
                ));
                0
            }
            None if redirect_to.is_some() || upstream_service.is_some() => 0,
            None => {
                problems.push(format!("web_api.products.{name}.port is required"));
                0
            }
        };
        let hostname = match entry.get("hostname").and_then(Value::as_str) {
            Some(hostname) if is_public_hostname(hostname) => hostname.to_string(),
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.hostname {other:?} is not a public host name"
                ));
                String::new()
            }
            None => {
                problems.push(format!("web_api.products.{name}.hostname is required"));
                String::new()
            }
        };
        // A path prefix a product is mounted at, under a hostname another
        // declaration owns. Written before the hostname bookkeeping below
        // because that bookkeeping now depends on it: one hostname has
        // exactly one owner and any number of mounts, and each mount holds a
        // distinct prefix.
        let path_prefix = match entry.get("path_prefix") {
            Some(Value::String(prefix)) if is_mount_prefix(prefix) => Some(prefix.clone()),
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.path_prefix {other} must be an absolute path with no trailing slash, like \"/docs\""
                ));
                None
            }
            None => None,
        };
        // A mount is an ordinary unit product; what it must not be is one of
        // the two hostname-only kinds. A redirect answers the whole hostname
        // and an upstream service forwards the whole hostname, so neither can
        // also be a path under someone else's.
        if path_prefix.is_some() {
            for (key, why) in [
                ("redirect_to", "a redirect answers a whole hostname"),
                (
                    "upstream_service",
                    "a hostname in front of a service forwards the whole hostname",
                ),
            ] {
                if entry.contains_key(key) {
                    problems.push(format!(
                        "web_api.products.{name} declares path_prefix and {key}: {why}, so it cannot also be a path under another product's hostname"
                    ));
                }
            }
        }
        if !hostname.is_empty() {
            match &path_prefix {
                // The owner: one per hostname, holding the record, the
                // certificate and the catch-all.
                None => {
                    if let Some(owner) = hostnames.insert(hostname.clone(), name.clone()) {
                        problems.push(format!(
                            "web_api.products.{name}.hostname {hostname:?} is already declared by {owner:?}"
                        ));
                    }
                }
                // A mount: it shares the hostname, so the only thing it must
                // not share is its prefix. Two mounts at one prefix would
                // render two `handle_path` blocks for one path and the first
                // would silently win.
                Some(prefix) => {
                    if let Some(owner) =
                        mounts.insert((hostname.clone(), prefix.clone()), name.clone())
                    {
                        problems.push(format!(
                            "web_api.products.{name} mounts {prefix:?} on {hostname:?}, which {owner:?} already mounts"
                        ));
                    }
                }
            }
        }
        let consumer = match entry.get("consumer").and_then(Value::as_str) {
            Some(consumer) if canonical_machine_name(consumer) => consumer.to_string(),
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.consumer {other:?} is not canonical"
                ));
                String::new()
            }
            // A redirect declares no consumer, and asking for one would be
            // asking for a vault identity nothing authenticates as.
            None if redirect_to.is_some() || upstream_service.is_some() => String::new(),
            None => {
                problems.push(format!("web_api.products.{name}.consumer is required"));
                String::new()
            }
        };
        let readyz = match entry.get("readyz") {
            Some(Value::String(path)) if path.starts_with('/') && !path.contains(' ') => {
                path.clone()
            }
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.readyz {other} must be an absolute request path"
                ));
                String::new()
            }
            None => "/".to_string(),
        };
        let edge = match entry.get("edge") {
            Some(Value::String(edge)) if WEB_API_EDGES.contains(&edge.as_str()) => edge.clone(),
            Some(other) => {
                problems.push(format!(
                    "web_api.products.{name}.edge {other} is not one of {WEB_API_EDGES:?}"
                ));
                String::new()
            }
            None => "stado".to_string(),
        };
        let mut env = BTreeMap::new();
        match entry.get("env") {
            Some(Value::Object(values)) => {
                for (key, value) in values {
                    match value.as_str() {
                        Some(value) if is_env_name(key) && !value.chars().any(char::is_control) => {
                            env.insert(key.clone(), value.to_string());
                        }
                        _ => problems.push(format!(
                            "web_api.products.{name}.env.{key} must be a plain string value"
                        )),
                    }
                }
            }
            Some(_) => problems.push(format!("web_api.products.{name}.env must be an object")),
            None => {}
        }
        let mut secrets = BTreeMap::new();
        match entry.get("secrets") {
            Some(Value::Object(values)) => {
                for (key, value) in values {
                    match value.as_str() {
                        Some(reference)
                            if is_env_name(key)
                                && parse_secret_reference(reference).is_some() =>
                        {
                            secrets.insert(key.clone(), reference.to_string());
                        }
                        _ => problems.push(format!(
                            "web_api.products.{name}.secrets.{key} must be an \"item#field\" reference"
                        )),
                    }
                }
            }
            Some(_) => problems.push(format!("web_api.products.{name}.secrets must be an object")),
            None => {}
        }
        let mut database = None;
        match entry.get("database") {
            Some(Value::Object(declared)) => {
                for key in declared.keys() {
                    if !matches!(key.as_str(), "name" | "field" | "variable") {
                        problems.push(format!(
                            "web_api.products.{name}.database contains unsupported key {key:?}"
                        ));
                    }
                }
                let declared_name = declared.get("name").and_then(Value::as_str).unwrap_or("");
                let field = declared.get("field").and_then(Value::as_str).unwrap_or("");
                let variable = declared
                    .get("variable")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let field_name = !field.is_empty()
                    && field.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                    });
                if !canonical_machine_name(declared_name) {
                    problems.push(format!(
                        "web_api.products.{name}.database.name is required and must be canonical"
                    ));
                } else if !field_name {
                    problems.push(format!(
                        "web_api.products.{name}.database.field is required and must be a field name"
                    ));
                } else if !is_env_name(variable) {
                    problems.push(format!(
                        "web_api.products.{name}.database.variable is required and must be an environment name"
                    ));
                } else {
                    database = Some(WebApiDatabase {
                        name: declared_name.to_string(),
                        field: field.to_string(),
                        variable: variable.to_string(),
                    });
                }
            }
            Some(_) => {
                problems.push(format!(
                    "web_api.products.{name}.database must be an object with name, field and variable"
                ));
            }
            None => {}
        }
        if problems.len() == start {
            products.insert(
                name.clone(),
                WebApiProduct {
                    host,
                    port,
                    hostname,
                    consumer,
                    readyz,
                    edge,
                    env,
                    secrets,
                    database,
                    redirect_to,
                    upstream_service,
                    path_prefix,
                },
            );
        }
    }
    // A mount is rendered inside the site block of the declaration that owns
    // its hostname, so a mount with no owner is a block with nowhere to go:
    // the edge would order no certificate for that name and the path would
    // answer from nothing. Checked after the loop because the owner may be
    // declared after the mount in the document.
    for ((hostname, prefix), name) in &mounts {
        if !hostnames.contains_key(hostname) {
            problems.push(format!(
                "web_api.products.{name} mounts {prefix:?} on {hostname:?}, which no declaration owns: one product must declare that hostname without a path_prefix, and it is the one that holds the record and the certificate"
            ));
        }
    }
    if problems.is_empty() {
        Ok(products)
    } else {
        Err(problems)
    }
}

static WEB_API_PRODUCTS: LazyLock<Result<BTreeMap<String, WebApiProduct>, Vec<String>>> =
    LazyLock::new(|| {
        match std::env::var("WC_WEB_API_PRODUCTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => serde_json::from_str::<Value>(&encoded)
                .map_err(|error| vec![format!("WC_WEB_API_PRODUCTS must be valid JSON: {error}")])
                .and_then(|parsed| parse_web_api_products(Some(&parsed))),
            None => parse_web_api_products(crate::config_file::get("web_api.products").as_ref()),
        }
    });

pub fn web_api_products() -> Result<&'static BTreeMap<String, WebApiProduct>, &'static [String]> {
    match &*WEB_API_PRODUCTS {
        Ok(products) => Ok(products),
        Err(problems) => Err(problems.as_slice()),
    }
}

/// The declared edge host: the fleet target that holds a public address and
/// terminates TLS for every `edge: "stado"` hostname.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebApiEdge {
    target: String,
    address: String,
    contact: String,
}

impl WebApiEdge {
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The public IPv4 address the product hostnames' A records point at.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The address Let's Encrypt sends expiry mail to.
    pub fn contact(&self) -> &str {
        &self.contact
    }
}

pub(crate) fn parse_web_api_edge(value: Option<&Value>) -> Result<WebApiEdge, Vec<String>> {
    let Some(Value::Object(entry)) = value else {
        return Err(vec![
            "web_api.edge must be an object with target, address and contact".to_string(),
        ]);
    };
    let mut problems = Vec::new();
    for key in entry.keys() {
        if !matches!(key.as_str(), "target" | "address" | "contact") {
            problems.push(format!("web_api.edge contains unsupported key {key:?}"));
        }
    }
    let target = entry.get("target").and_then(Value::as_str).unwrap_or("");
    if !canonical_machine_name(target) {
        problems.push("web_api.edge.target is required and must be a canonical target".to_string());
    }
    let address = entry.get("address").and_then(Value::as_str).unwrap_or("");
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        problems.push("web_api.edge.address is required and must be an IPv4 address".to_string());
    }
    let contact = entry.get("contact").and_then(Value::as_str).unwrap_or("");
    if !contact.contains('@') || contact.chars().any(char::is_whitespace) {
        problems.push("web_api.edge.contact is required and must be a mail address".to_string());
    }
    if problems.is_empty() {
        Ok(WebApiEdge {
            target: target.to_string(),
            address: address.to_string(),
            contact: contact.to_string(),
        })
    } else {
        Err(problems)
    }
}

static WEB_API_EDGE: LazyLock<Result<WebApiEdge, Vec<String>>> =
    LazyLock::new(|| parse_web_api_edge(crate::config_file::get("web_api.edge").as_ref()));

pub fn web_api_edge() -> Result<&'static WebApiEdge, &'static [String]> {
    match &*WEB_API_EDGE {
        Ok(edge) => Ok(edge),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub const SERVICE_API_VERIFIER_CONSUMER: &str = "stado-service-api-verifier";
pub const SERVICE_API_ACTIONS: &[&str] = &["status", "restart", "promote", "reconcile"];
pub const ACTIVE_DEPLOYED_SERVICES: &[&str] = &["com.wisent.weles-api", "image-video-router"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceDeployer {
    item: String,
    consumer: String,
    services: Vec<String>,
    actions: Vec<String>,
}

impl ServiceDeployer {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn consumer(&self) -> &str {
        &self.consumer
    }

    pub fn services(&self) -> &[String] {
        &self.services
    }

    pub fn actions(&self) -> &[String] {
        &self.actions
    }

    pub fn allows(&self, service: &str, action: &str) -> bool {
        self.services.iter().any(|configured| configured == service)
            && self.actions.iter().any(|configured| configured == action)
    }
}

pub(crate) fn parse_service_deployers(
    value: Option<&Value>,
) -> Result<BTreeMap<String, ServiceDeployer>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "service_api.deployers must be a non-empty product-to-deployer mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec![
            "service_api.deployers must not be empty; managed-service routes fail closed"
                .to_string(),
        ]);
    }

    let canonical = |value: &str| {
        !value.is_empty()
            && value.trim() == value
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    let mut problems = Vec::new();
    let mut deployers = BTreeMap::new();
    let mut items = BTreeSet::new();
    let mut services_seen = BTreeSet::new();
    for (product, raw_entry) in entries {
        let mut entry_valid = true;
        if !canonical(product) {
            problems.push(format!(
                "service_api.deployers key {product:?} is not a canonical product name"
            ));
            entry_valid = false;
        }
        let Some(entry) = raw_entry.as_object() else {
            problems.push(format!(
                "service_api.deployers.{product} must contain consumer, item, services, and actions"
            ));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "consumer" | "item" | "services" | "actions") {
                problems.push(format!(
                    "service_api.deployers.{product} contains unsupported key {key:?}"
                ));
                entry_valid = false;
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let consumer = entry
            .get("consumer")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !canonical(item) || !item.ends_with("-deployer") {
            problems.push(format!(
                "service_api.deployers.{product}.item must name one canonical *-deployer item"
            ));
            entry_valid = false;
        }
        if consumer != item {
            problems.push(format!(
                "service_api.deployers.{product}.consumer must equal its exact item {item:?}"
            ));
            entry_valid = false;
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "service_api.deployers maps more than one product to item {item:?}"
            ));
            entry_valid = false;
        }
        let services = match entry.get("services") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut parsed = Vec::with_capacity(values.len());
                for value in values {
                    let Some(service) = value.as_str() else {
                        problems.push(format!(
                            "service_api.deployers.{product}.services entries must be strings"
                        ));
                        entry_valid = false;
                        continue;
                    };
                    if !canonical(service) && service != "com.wisent.weles-api" {
                        problems.push(format!(
                            "service_api.deployers.{product}.services contains non-canonical {service:?}"
                        ));
                        entry_valid = false;
                    }
                    if !services_seen.insert(service.to_string()) {
                        problems.push(format!(
                            "service {service:?} is mapped to more than one deployer"
                        ));
                        entry_valid = false;
                    }
                    parsed.push(service.to_string());
                }
                parsed
            }
            _ => {
                problems.push(format!(
                    "service_api.deployers.{product}.services must be a non-empty string array"
                ));
                entry_valid = false;
                Vec::new()
            }
        };
        let actions = match entry.get("actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut parsed = Vec::with_capacity(values.len());
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(action) = value.as_str() else {
                        problems.push(format!(
                            "service_api.deployers.{product}.actions entries must be strings"
                        ));
                        entry_valid = false;
                        continue;
                    };
                    if !SERVICE_API_ACTIONS.contains(&action) || !seen.insert(action.to_string()) {
                        problems.push(format!(
                            "service_api.deployers.{product}.actions contains unsupported or duplicate {action:?}"
                        ));
                        entry_valid = false;
                    }
                    parsed.push(action.to_string());
                }
                parsed
            }
            _ => {
                problems.push(format!(
                    "service_api.deployers.{product}.actions must be a non-empty string array"
                ));
                entry_valid = false;
                Vec::new()
            }
        };
        if entry_valid {
            deployers.insert(
                product.to_string(),
                ServiceDeployer {
                    item: item.to_string(),
                    consumer: consumer.to_string(),
                    services,
                    actions,
                },
            );
        }
    }
    for &required in ACTIVE_DEPLOYED_SERVICES {
        if !services_seen.contains(required) {
            problems.push(format!(
                "service_api.deployers is missing active service {required:?}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(deployers)
    } else {
        Err(problems)
    }
}

static SERVICE_API_DEPLOYERS: LazyLock<Result<BTreeMap<String, ServiceDeployer>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_SERVICE_API_DEPLOYERS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_SERVICE_API_DEPLOYERS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("service_api.deployers"),
        };
        parse_service_deployers(configured.as_ref())
    });
static SERVICE_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SERVICE_SKARBIEC_URL",
        "service_api.skarbiec.url",
        skarbiec_url(),
    )
});
static SERVICE_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SERVICE_SKARBIEC_CONSUMER",
        "service_api.skarbiec.consumer",
        SERVICE_API_VERIFIER_CONSUMER,
    )
});
static SERVICE_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-service-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_SERVICE_SKARBIEC_TOKEN_FILE",
        "service_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub const RATE_LIMIT_API_VERIFIER_CONSUMER: &str = "stado-rate-limit-api-verifier";

static RATE_LIMIT_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RATE_LIMIT_SKARBIEC_URL",
        "rate_limit.skarbiec.url",
        skarbiec_url(),
    )
});
static RATE_LIMIT_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RATE_LIMIT_SKARBIEC_CONSUMER",
        "rate_limit.skarbiec.consumer",
        RATE_LIMIT_API_VERIFIER_CONSUMER,
    )
});
static RATE_LIMIT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-rate-limit-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_RATE_LIMIT_SKARBIEC_TOKEN_FILE",
        "rate_limit.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub const INTEGRATION_API_VERIFIER_CONSUMER: &str = "stado-integration-api-verifier";
/// Domains reachable through `/api/integration/`. Stado serves only the
/// read-only fleet projection; every product-integration domain moved to the
/// private `wisent-integrations` service together with its client grants.
pub const INTEGRATION_CLIENT_DOMAINS: &[&str] = &["enterprise"];

/// Domains whose provider grant Stado itself resolves. `most` is the SMS
/// escalation path the monitor uses for alerting, so its Twilio credential
/// stays a fleet concern.
pub const INTEGRATION_PROVIDER_DOMAINS: &[&str] = &["most"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationClient {
    item: String,
    allowed_actions: Vec<String>,
}

impl IntegrationClient {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows(&self, domain: &str, action: &str) -> bool {
        self.allowed_actions.iter().any(|allowed| {
            allowed
                .split_once('/')
                .is_some_and(|(allowed_domain, allowed_action)| {
                    allowed_domain == domain && allowed_action == action
                })
        })
    }

    pub fn allowed_actions(&self) -> &[String] {
        &self.allowed_actions
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationProvider {
    consumer: String,
    token_file: String,
    items: Vec<String>,
}

impl IntegrationProvider {
    pub fn consumer(&self) -> &str {
        &self.consumer
    }

    pub fn token_file(&self) -> &str {
        &self.token_file
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }
}

fn canonical_integration_component(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
        })
}

pub(crate) fn parse_integration_clients(
    value: Option<&Value>,
) -> Result<BTreeMap<String, IntegrationClient>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "integration.clients must be a non-empty exact client mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["integration.clients must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut clients = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_integration_component(name) || name.contains('.') {
            problems.push(format!("integration.clients key {name:?} is not canonical"));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!("integration.clients.{name} must be an object"));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "item" | "allowed_actions") {
                problems.push(format!(
                    "integration.clients.{name} contains unsupported key {key:?}"
                ));
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !canonical_integration_component(item) || !item.ends_with("-integration-api") {
            problems.push(format!(
                "integration.clients.{name}.item must be a canonical product-specific *-integration-api item"
            ));
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "integration.clients maps more than one client to item {item:?}"
            ));
        }
        let mut allowed_actions = Vec::new();
        match entry.get("allowed_actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(value) = value.as_str() else {
                        problems.push(format!(
                            "integration.clients.{name}.allowed_actions entries must be strings"
                        ));
                        continue;
                    };
                    let canonical = value.split_once('/').is_some_and(|(domain, action)| {
                        !domain.contains('.')
                            && canonical_integration_component(domain)
                            && INTEGRATION_CLIENT_DOMAINS.contains(&domain)
                            && canonical_integration_component(action)
                    });
                    if !canonical || !seen.insert(value) {
                        problems.push(format!(
                            "integration.clients.{name}.allowed_actions contains invalid or duplicate {value:?}"
                        ));
                        continue;
                    }
                    allowed_actions.push(value.to_string());
                }
            }
            _ => problems.push(format!(
                "integration.clients.{name}.allowed_actions must be a non-empty array"
            )),
        }
        if problems.len() == start {
            clients.insert(
                name.to_string(),
                IntegrationClient {
                    item: item.to_string(),
                    allowed_actions,
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(clients)
    } else {
        Err(problems)
    }
}

pub(crate) fn parse_integration_providers(
    value: Option<&Value>,
) -> Result<BTreeMap<String, IntegrationProvider>, Vec<String>> {
    let entries = match value {
        None => return Ok(BTreeMap::new()),
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            return Err(vec![
                "integration.providers must be an exact domain mapping".to_string(),
            ])
        }
    };
    if entries.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut problems = Vec::new();
    let mut providers = BTreeMap::new();
    let mut consumers = BTreeSet::new();
    let mut token_files = BTreeSet::new();
    let mut all_items = BTreeSet::new();
    for (domain, raw) in entries {
        let start = problems.len();
        if !canonical_integration_component(domain) || domain.contains('.') {
            problems.push(format!(
                "integration.providers key {domain:?} is not canonical"
            ));
        }
        if !INTEGRATION_PROVIDER_DOMAINS.contains(&domain.as_str()) {
            problems.push(format!(
                "integration.providers contains unsupported domain {domain:?}"
            ));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!("integration.providers.{domain} must be an object"));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "consumer" | "token_file" | "items") {
                problems.push(format!(
                    "integration.providers.{domain} contains unsupported key {key:?}"
                ));
            }
        }
        let consumer = entry
            .get("consumer")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_consumer = format!("stado-{domain}-integration-provider");
        if consumer != expected_consumer {
            problems.push(format!(
                "integration.providers.{domain}.consumer must be {expected_consumer:?}"
            ));
        }
        if !consumers.insert(consumer.to_string()) {
            problems.push(format!(
                "integration.providers reuses consumer {consumer:?}"
            ));
        }
        let token_file = entry
            .get("token_file")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if token_file.trim().is_empty() {
            problems.push(format!(
                "integration.providers.{domain}.token_file is required"
            ));
        }
        let token_file = expand_tilde(token_file).to_string_lossy().into_owned();
        if !token_files.insert(token_file.clone()) {
            problems.push(format!(
                "integration.providers reuses token_file for domain {domain:?}"
            ));
        }
        let mut items = Vec::new();
        match entry.get("items") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(item) = value.as_str() else {
                        problems.push(format!(
                            "integration.providers.{domain}.items entries must be strings"
                        ));
                        continue;
                    };
                    if !canonical_integration_component(item)
                        || item.ends_with("-integration-api")
                        || !seen.insert(item)
                        || !all_items.insert(item)
                    {
                        problems.push(format!(
                            "integration.providers.{domain}.items contains invalid, duplicate, or cross-domain item {item:?}"
                        ));
                        continue;
                    }
                    items.push(item.to_string());
                }
            }
            _ => problems.push(format!(
                "integration.providers.{domain}.items must be a non-empty array"
            )),
        }
        if problems.len() == start {
            providers.insert(
                domain.to_string(),
                IntegrationProvider {
                    consumer: consumer.to_string(),
                    token_file,
                    items,
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(providers)
    } else {
        Err(problems)
    }
}

static INTEGRATION_CLIENTS: LazyLock<Result<BTreeMap<String, IntegrationClient>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_INTEGRATION_CLIENTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_INTEGRATION_CLIENTS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("integration.clients"),
        };
        parse_integration_clients(configured.as_ref())
    });
static INTEGRATION_PROVIDERS: LazyLock<Result<BTreeMap<String, IntegrationProvider>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_INTEGRATION_PROVIDERS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_INTEGRATION_PROVIDERS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("integration.providers"),
        };
        parse_integration_providers(configured.as_ref())
    });
static INTEGRATION_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_INTEGRATION_SKARBIEC_URL",
        "integration.skarbiec.url",
        skarbiec_url(),
    )
});
static INTEGRATION_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_INTEGRATION_SKARBIEC_CONSUMER",
        "integration.skarbiec.consumer",
        INTEGRATION_API_VERIFIER_CONSUMER,
    )
});
static INTEGRATION_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-integration-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_INTEGRATION_SKARBIEC_TOKEN_FILE",
        "integration.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});
static INTEGRATION_PROVIDER_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_INTEGRATION_PROVIDER_SKARBIEC_URL",
        "integration.provider_skarbiec.url",
        skarbiec_url(),
    )
});

/// Billing sources queried by the collector. This is independent from compute
/// provider enablement: an account may stay fenced for provisioning while its
/// spend and grant state remain monitored.
pub fn billing_providers() -> &'static [String] {
    &BILLING_PROVIDERS
}

/// BigQuery billing export dataset (env `WC_BILLING_DATASET`).
///
/// Billing-credits collector. Each tick the Cloud Function reads the GCP
/// BigQuery billing export (gross / credits-applied / net + per-credit
/// cumulative + 7-day burn) and the Azure available-credit balance, then
/// writes gs://<BUCKET>/billing_health/credits.json (same convention as
/// host_health/<host>.json). The export table is account-specific; it is
/// resolved from env so a different billing account only needs a redeploy
/// env change, never a code edit. Dataset/table default to the live
/// wisent-480400 export confirmed present 2026-05-16.
pub fn billing_dataset() -> &'static str {
    BILLING_DATASET.as_str()
}

/// BigQuery billing export table (env `WC_BILLING_TABLE`).
pub fn billing_table() -> &'static str {
    BILLING_TABLE.as_str()
}

/// Net-spend alert threshold in USD (env `WC_BILLING_NET_ALERT_USD`). A day
/// whose net_cost (gross + credits, credits are negative) exceeds this
/// means the promotion credit no longer fully covers spend — i.e. it is
/// exhausted or rate-capped. This is the depletion signal; it needs no
/// knowledge of the original grant ceiling (which no GCP API exposes).
pub fn billing_net_alert_usd() -> f64 {
    *BILLING_NET_ALERT_USD
}

/// Skarbiec item holding the Azure billing service principal as
/// `{"tenant_id","client_id","client_secret", ...}`. The item name is selected
/// by `WC_AZURE_BILLING_SECRET`; its value has no alternative source.
pub fn azure_billing_secret() -> &'static str {
    AZURE_BILLING_SECRET.as_str()
}

/// Skarbiec item holding the Azure provider service principal as
/// `{"tenant_id","client_id","client_secret"}`, used by hosts that have no
/// Azure managed identity. The item name is selected by `WC_AZURE_SECRET`.
///
/// A managed identity is still preferred and tried first; this exists because
/// the control plane runs on hardware outside Azure, where IMDS answers
/// nothing and Azure Blob would otherwise be unreachable.
pub fn azure_provider_secret() -> &'static str {
    AZURE_PROVIDER_SECRET.as_str()
}

/// Loopback URL of the separate Skarbiec service.
pub fn skarbiec_url() -> &'static str {
    SKARBIEC_URL.as_str()
}

/// Scoped Skarbiec grant consumer name.
pub fn skarbiec_consumer() -> &'static str {
    SKARBIEC_CONSUMER.as_str()
}

/// Owner-only file containing the scoped Skarbiec grant.
pub fn skarbiec_token_file() -> &'static str {
    SKARBIEC_TOKEN_FILE.as_str()
}

/// Exact private product namespace policies accepted by the object gateway.
pub fn object_api_namespaces(
) -> Result<&'static BTreeMap<String, ObjectApiNamespace>, &'static [String]> {
    match &*OBJECT_API_NAMESPACES {
        Ok(namespaces) => Ok(namespaces),
        Err(problems) => Err(problems.as_slice()),
    }
}
/// Exact item set visible to the dashboard's least-privilege verifier.
///
/// Kept in one function because startup validation and remote reconciliation
/// must agree byte-for-byte: adding a protected route to only one side closes
/// the whole boundary as either missing or over-broad.
pub fn object_verifier_items(
    namespaces: &BTreeMap<String, ObjectApiNamespace>,
) -> BTreeSet<String> {
    namespaces
        .values()
        .map(|policy| policy.item().to_string())
        .chain(std::iter::once(HOST_HEALTH_API_ITEM.to_string()))
        .collect()
}

/// Policy for one canonical namespace. Invalid aggregate configuration fails
/// closed for every namespace rather than partially enabling the valid rows.
pub fn object_api_namespace(namespace: &str) -> Option<&'static ObjectApiNamespace> {
    object_api_namespaces().ok()?.get(namespace)
}

/// Skarbiec endpoint used only by the product-token verifier grant.
pub fn object_skarbiec_url() -> &'static str {
    OBJECT_SKARBIEC_URL.as_str()
}

/// Dedicated least-privilege consumer that can read exactly the mapped items.
pub fn object_skarbiec_consumer() -> &'static str {
    OBJECT_SKARBIEC_CONSUMER.as_str()
}

/// Owner-only grant file for the dedicated object-token verifier.
pub fn object_skarbiec_token_file() -> &'static str {
    OBJECT_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn release_api_publishers(
) -> Result<&'static BTreeMap<String, ReleasePublisher>, &'static [String]> {
    match &*RELEASE_API_PUBLISHERS {
        Ok(publishers) => Ok(publishers),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn release_publisher_for_key(key: &str) -> Option<&'static ReleasePublisher> {
    release_api_publishers()
        .ok()?
        .values()
        .find(|publisher| publisher.allows_key(key))
}

pub fn release_publisher_for_list(prefix: &str) -> Option<(&'static ReleasePublisher, String)> {
    release_api_publishers()
        .ok()?
        .values()
        .find_map(|publisher| {
            publisher
                .authorized_list_prefix(prefix)
                .map(|authorized| (publisher, authorized))
        })
}

pub fn release_skarbiec_url() -> &'static str {
    RELEASE_SKARBIEC_URL.as_str()
}

pub fn release_skarbiec_consumer() -> &'static str {
    RELEASE_SKARBIEC_CONSUMER.as_str()
}

pub fn release_skarbiec_token_file() -> &'static str {
    RELEASE_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn release_signing_skarbiec_consumer() -> &'static str {
    RELEASE_SIGNING_SKARBIEC_CONSUMER.as_str()
}

pub fn release_signing_skarbiec_token_file() -> &'static str {
    RELEASE_SIGNING_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn machine_api_clients(
) -> Result<&'static BTreeMap<String, MachineApiClient>, &'static [String]> {
    match &*MACHINE_API_CLIENTS {
        Ok(clients) => Ok(clients),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn registry_api_clients(
) -> Result<&'static BTreeMap<String, RegistryApiClient>, &'static [String]> {
    match &*REGISTRY_API_CLIENTS {
        Ok(clients) => Ok(clients),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn registry_skarbiec_url() -> &'static str {
    REGISTRY_SKARBIEC_URL.as_str()
}

pub fn registry_skarbiec_consumer() -> &'static str {
    REGISTRY_SKARBIEC_CONSUMER.as_str()
}

pub fn registry_skarbiec_token_file() -> &'static str {
    REGISTRY_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn machine_skarbiec_url() -> &'static str {
    MACHINE_SKARBIEC_URL.as_str()
}

pub fn machine_skarbiec_consumer() -> &'static str {
    MACHINE_SKARBIEC_CONSUMER.as_str()
}

pub fn machine_skarbiec_token_file() -> &'static str {
    MACHINE_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn service_api_deployers(
) -> Result<&'static BTreeMap<String, ServiceDeployer>, &'static [String]> {
    match &*SERVICE_API_DEPLOYERS {
        Ok(deployers) => Ok(deployers),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn service_deployer_for(service: &str, action: &str) -> Option<&'static ServiceDeployer> {
    service_api_deployers()
        .ok()?
        .values()
        .find(|deployer| deployer.allows(service, action))
}

pub fn service_skarbiec_url() -> &'static str {
    SERVICE_SKARBIEC_URL.as_str()
}

pub fn service_skarbiec_consumer() -> &'static str {
    SERVICE_SKARBIEC_CONSUMER.as_str()
}

pub fn service_skarbiec_token_file() -> &'static str {
    SERVICE_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn rate_limit_skarbiec_url() -> &'static str {
    RATE_LIMIT_SKARBIEC_URL.as_str()
}

pub fn rate_limit_skarbiec_consumer() -> &'static str {
    RATE_LIMIT_SKARBIEC_CONSUMER.as_str()
}

pub fn rate_limit_skarbiec_token_file() -> &'static str {
    RATE_LIMIT_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn integration_clients(
) -> Result<&'static BTreeMap<String, IntegrationClient>, &'static [String]> {
    match &*INTEGRATION_CLIENTS {
        Ok(clients) => Ok(clients),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn integration_providers(
) -> Result<&'static BTreeMap<String, IntegrationProvider>, &'static [String]> {
    match &*INTEGRATION_PROVIDERS {
        Ok(providers) => Ok(providers),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn integration_provider(domain: &str) -> Option<&'static IntegrationProvider> {
    integration_providers().ok()?.get(domain)
}

pub fn integration_skarbiec_url() -> &'static str {
    INTEGRATION_SKARBIEC_URL.as_str()
}

pub fn integration_skarbiec_consumer() -> &'static str {
    INTEGRATION_SKARBIEC_CONSUMER.as_str()
}

pub fn integration_skarbiec_token_file() -> &'static str {
    INTEGRATION_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn integration_provider_skarbiec_url() -> &'static str {
    INTEGRATION_PROVIDER_SKARBIEC_URL.as_str()
}

/// Skarbiec endpoint reachable by workload agents. Cloud agents require HTTPS;
/// a device-local agent may leave this empty and use [`skarbiec_url`].
pub fn agent_skarbiec_url() -> &'static str {
    AGENT_SKARBIEC_URL.as_str()
}

/// Consumer name of the dedicated workload-agent grant. Its exact read scopes
/// are minted from the workloads this deployment is allowed to execute.
pub fn agent_skarbiec_consumer() -> &'static str {
    AGENT_SKARBIEC_CONSUMER.as_str()
}

/// Owner-only file containing the workload-agent grant. Cloud deployment
/// delivers the token to VM tmpfs; the local control plane uses a separate
/// device grant rather than reusing its coordinator grant.
pub fn agent_skarbiec_token_file() -> &'static str {
    AGENT_SKARBIEC_TOKEN_FILE.as_str()
}
/// Exact Skarbiec items visible to workload agents. The coordinator verifies
/// that the scoped grant can list neither fewer nor more items before dispatch.
pub fn agent_skarbiec_items() -> &'static [String] {
    &AGENT_SKARBIEC_ITEMS
}

/// Exact workload-visible `item#field` references. Infrastructure items may
/// still be present in [`agent_skarbiec_items`] for trusted agent internals,
/// but a queued job can resolve only entries in this second, field-level list.
pub fn agent_skarbiec_secret_fields() -> &'static [String] {
    &AGENT_SKARBIEC_SECRET_FIELDS
}

/// HTTPS Skarbiec endpoint of the backend business-messaging grant, which
/// Stado reads only to resolve the operator-session Supabase project.
pub fn backend_messaging_skarbiec_url() -> &'static str {
    BACKEND_MESSAGING_SKARBIEC_URL.as_str()
}

/// Dedicated consumer whose grant contains only backend messaging providers.
pub fn backend_messaging_skarbiec_consumer() -> &'static str {
    BACKEND_MESSAGING_SKARBIEC_CONSUMER.as_str()
}

/// Owner-only grant file for the backend business-messaging grant.
pub fn backend_messaging_skarbiec_token_file() -> &'static str {
    BACKEND_MESSAGING_SKARBIEC_TOKEN_FILE.as_str()
}

/// Exact provider and device-registry items visible to the messaging grant.
pub fn backend_messaging_skarbiec_items() -> &'static [String] {
    &BACKEND_MESSAGING_SKARBIEC_ITEMS
}

/// Whether a job may project one exact Skarbiec field into its environment.
/// Matching without allocating keeps this check cheap on every admission path.
pub fn agent_secret_reference_allowed(item: &str, field: &str) -> bool {
    AGENT_SKARBIEC_SECRET_FIELDS.iter().any(|entry| {
        entry
            .split_once('#')
            .is_some_and(|(allowed_item, allowed_field)| {
                allowed_item == item && allowed_field == field
            })
    })
}

/// In-process cache TTL for the model policy loaded through the configured
/// [`crate::queue::JobStorage`] adapter.
pub const MODEL_POLICY_TTL_S: u64 = 300;

/// Co-schedule and cost-policy flags loaded from the provider-neutral
/// `config/model_overrides.json` object in the configured queue store.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct ModelPolicy {
    pub exclusive: Vec<String>,
    pub local_only: Vec<String>,
}

#[derive(Debug, Default)]
struct ModelPolicyCache {
    policy: ModelPolicy,
    fetched_at: Option<Instant>,
}

static MODEL_POLICY: LazyLock<RwLock<ModelPolicyCache>> =
    LazyLock::new(|| RwLock::new(ModelPolicyCache::default()));

/// Refresh the shared policy when its TTL expires. A missing blob means an
/// intentionally empty policy; transport or JSON errors leave the last good
/// value untouched and are returned to the caller for logging.
pub async fn refresh_model_policy(
    store: &crate::queue::JobStorage,
) -> Result<ModelPolicy, crate::queue::StorageError> {
    {
        let cache = MODEL_POLICY
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache
            .fetched_at
            .is_some_and(|at| at.elapsed().as_secs() < MODEL_POLICY_TTL_S)
        {
            return Ok(cache.policy.clone());
        }
    }

    let policy = match store.download_text("config/model_overrides.json").await? {
        Some(raw) => serde_json::from_str::<ModelPolicy>(&raw)?,
        None => ModelPolicy::default(),
    };
    let mut cache = MODEL_POLICY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.policy = policy.clone();
    cache.fetched_at = Some(Instant::now());
    Ok(policy)
}

/// Last successfully fetched policy.
pub fn model_policy() -> ModelPolicy {
    MODEL_POLICY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .policy
        .clone()
}

/// True when `model` must run with exclusive GPU ownership.
pub fn is_exclusive_model(model: &str) -> bool {
    model_policy()
        .exclusive
        .iter()
        .any(|candidate| candidate == model)
}

/// True when `model` is restricted to local execution.
pub fn is_local_only_model(model: &str) -> bool {
    model_policy()
        .local_only
        .iter()
        .any(|candidate| candidate == model)
}

/// Compute API base URL (env `COMPUTE_API_URL`). Python resolves this at
/// import time in `stado/queue/submit.py` (`COMPUTE_API`).
static COMPUTE_API: LazyLock<String> = LazyLock::new(|| {
    std::env::var("COMPUTE_API_URL").unwrap_or_else(|_| "https://compute.wisent.com".to_string())
});

/// Base URL of the compute.wisent.com API (env `COMPUTE_API_URL`).
pub fn compute_api() -> &'static str {
    COMPUTE_API.as_str()
}

/// Estimate GPU memory needed from a command string.
///
/// Port of `stado/config.py::estimate_gpu_memory`. The model-name regex
/// extraction (`--model\s+(\S+)`, quote-stripped) is byte-faithful.
/// Python's call is sync over a GCS scan (`sizing.observed_vram_gb` /
/// `sizing.smallest_live_vram`); here the scan is async and goes through
/// the passed [`JobStorage`] + [`crate::sizing::Sizing`] cache holder.
/// A storage outage propagates (Python: the sizing rebuild raises), which
/// the submit path surfaces as a submit error.
pub async fn estimate_gpu_memory(
    command: &str,
    sizing: &crate::sizing::Sizing,
    store: &crate::queue::JobStorage,
) -> Result<i64, crate::queue::StorageError> {
    static MODEL_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"--model\s+(\S+)").expect("static regex compiles"));
    let Some(caps) = MODEL_RE.captures(command) else {
        return Ok(0);
    };
    let model = caps[1].trim_matches(['\'', '"']);

    // Sizing is PURELY the measured peak. No params formula, no per-model
    // constant, no multiplier, no hand-written tier ladder — all forbidden
    // hardcoded guesses. observed_vram_gb returns the min real nvidia-smi
    // peak_vram_gb the fleet has recorded for this model.
    if let Some(measured) = sizing.observed_vram_gb(store, model).await? {
        return Ok(measured);
    }

    // No measurement yet: do NOT fabricate a number. Start on the
    // smallest GPU that ACTUALLY EXISTS in the fleet right now (read from
    // live capacity broadcasts, not a catalog). If it OOMs there,
    // slots.advance_slot -> sizing.escalate_on_oom moves it to the next
    // larger REAL fleet GPU, repeating until it runs; that run's measured
    // nvidia-smi peak then sizes every later job of this model. If no
    // live agent is broadcasting, return 0 (unsized) rather than invent a
    // size — the job waits for a real GPU to appear.
    Ok(sizing.smallest_live_vram(store).await?.unwrap_or(0))
}

/// Return (machine_type, accel_type) for the given memory requirement.
///
/// If gpu_mem_gb exceeds every tier in GPU_SIZING, returns the LARGEST
/// available tier rather than ("", ""). The previous behavior produced an
/// empty machine_type that the GCE create_instance call rejected with
/// 'Machine type with name "" does not exist', wedging the job. Sending it
/// to the largest tier means the in-VM workload may still OOM, but that's a
/// clearer failure mode than a malformed instance request.
pub fn lookup_instance_type(provider: &str, gpu_mem_gb: i64) -> (&'static str, &'static str) {
    let Some(sizing) = GPU_SIZING.get(provider) else {
        return ("", "");
    };
    if let Some((_, spec)) = sizing.range(gpu_mem_gb..).next() {
        return *spec;
    }
    sizing
        .iter()
        .next_back()
        .map(|(_, spec)| *spec)
        .unwrap_or(("", ""))
}
