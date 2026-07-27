//! Configuration and constants.
//!
//! Port of `stado/config.py`. Python resolves these at import time; Rust has
//! no import-time hooks, so every value that goes through env/config-file
//! resolution is exposed as an accessor function backed by `LazyLock`
//! (resolved once, on first use). Plain compile-time constants keep their
//! Python names as `pub const`. Env var names are byte-identical to Python.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::Instant;

use crate::catalog::GPU_SIZING;
use crate::config_file::{expand_tilde, resolve as cfg, resolve_list as cfg_list};

static PROJECT: LazyLock<String> = LazyLock::new(|| cfg("GCP_PROJECT", "project", "wisent-480400"));
static BUCKET: LazyLock<String> = LazyLock::new(|| cfg("WC_BUCKET", "storage.gcs.bucket", "stado"));
static REGION: LazyLock<String> = LazyLock::new(|| cfg("GCP_REGION", "region", "us-central1"));
static ALERTS_TOPIC: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_ALERTS_TOPIC")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            cfg("", "alerts.topic", &format!("projects/{}/topics/stado-alerts", project()))
        })
});

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

static REGIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    cfg_list(
        "GCP_REGIONS",
        "regions",
        &["us-central1", "europe-west4", "us-east1", "us-east4", "us-east5"],
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
static DASHBOARD_REFRESH_SECONDS: LazyLock<i64> =
    LazyLock::new(|| cfg_i64("WC_DASHBOARD_REFRESH_SECONDS", "dashboard.refresh_seconds", "10"));
static DASHBOARD_AGENT_FRESH_SECONDS: LazyLock<i64> = LazyLock::new(|| {
    cfg_i64("WC_DASHBOARD_AGENT_FRESH_SECONDS", "dashboard.agent_fresh_seconds", "180")
});

/// Dashboard HTTP server bind address (env `WC_DASHBOARD_BIND`). Bind to
/// all interfaces so a tailscale serve front-end can reach it; the host is
/// firewalled to the tailnet anyway.
pub fn dashboard_bind() -> &'static str {
    DASHBOARD_BIND.as_str()
}

/// Dashboard HTTP port (env `WC_DASHBOARD_PORT`).
pub fn dashboard_port() -> i64 {
    *DASHBOARD_PORT
}

/// Dashboard auto-refresh interval (env `WC_DASHBOARD_REFRESH_SECONDS`).
pub fn dashboard_refresh_seconds() -> i64 {
    *DASHBOARD_REFRESH_SECONDS
}

/// Capacity blob is "live" if its published_at is within this many seconds
/// (env `WC_DASHBOARD_AGENT_FRESH_SECONDS`).
pub fn dashboard_agent_fresh_seconds() -> i64 {
    *DASHBOARD_AGENT_FRESH_SECONDS
}

/// Deployment gate for the dashboard (env `STADO_DEPLOYMENT_ID`), trimmed.
/// When set, the dashboard requires Supabase RLS Bearer auth and relaxes
/// the Host-header DNS-rebinding guard for authenticated HTTPS reverse
/// proxies. Read per call (Python reads `os.environ` at request time), not
/// cached in a `LazyLock`.
pub fn stado_deployment_id() -> String {
    std::env::var("STADO_DEPLOYMENT_ID").unwrap_or_default().trim().to_string()
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
static AZURE_SUBSCRIPTION_ID: LazyLock<String> =
    LazyLock::new(|| cfg("AZURE_SUBSCRIPTION_ID", "azure.subscription_id", ""));
static AZURE_RESOURCE_GROUP: LazyLock<String> =
    LazyLock::new(|| cfg("AZURE_RESOURCE_GROUP", "azure.resource_group", "wisent-compute"));
static AZURE_LOCATIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    cfg_list("AZURE_LOCATIONS", "azure.locations", &["eastus", "westus3", "westus2", "northeurope"])
});
static AZURE_VNET: LazyLock<String> =
    LazyLock::new(|| cfg("AZURE_VNET", "azure.vnet", "wisent-compute-vnet"));
static AZURE_SUBNET: LazyLock<String> =
    LazyLock::new(|| cfg("AZURE_SUBNET", "azure.subnet", "wisent-compute-subnet"));
static AZURE_NSG: LazyLock<String> =
    LazyLock::new(|| cfg("AZURE_NSG", "azure.nsg", "wisent-compute-nsg"));
static AZURE_IMAGE_URN: LazyLock<String> = LazyLock::new(|| {
    cfg("AZURE_IMAGE_URN", "azure.image_urn", "microsoft-dsvm:ubuntu-hpc:2204:latest")
});
static AZURE_VM_USERNAME: LazyLock<String> =
    LazyLock::new(|| cfg("AZURE_VM_USERNAME", "azure.vm_username", "wisent"));
static AZURE_SSH_PUBLIC_KEY: LazyLock<String> =
    LazyLock::new(|| cfg("AZURE_SSH_PUBLIC_KEY", "azure.ssh_public_key", ""));

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

// AWS (parallel to GCP). Python aws.py reads these straight from
// os.environ (no config-file keys), so these accessors do the same — no
// `cfg(...)` fallback. Python resolves them per create_instance call;
// here LazyLock freezes them on first use, which is equivalent in
// practice (env does not change mid-process).
static AWS_REGION: LazyLock<String> =
    LazyLock::new(|| std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string()));
static AWS_SECURITY_GROUP: LazyLock<String> =
    LazyLock::new(|| std::env::var("AWS_SECURITY_GROUP").unwrap_or_default());
static AWS_IAM_PROFILE: LazyLock<String> =
    LazyLock::new(|| std::env::var("AWS_IAM_PROFILE").unwrap_or_else(|_| "stado-agent".to_string()));
static AWS_AMI_ID: LazyLock<String> =
    LazyLock::new(|| std::env::var("AWS_AMI_ID").unwrap_or_default().trim().to_string());

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

static WC_PROVIDERS: LazyLock<Vec<String>> =
    LazyLock::new(|| cfg_list("WC_PROVIDERS", "providers", &["gcp"]));

/// Multi-provider dispatch (env `WC_PROVIDERS`, comma-separated).
/// Coordinator and Cloud Function tick iterate this list, calling
/// check_running_jobs / reap_dead_agents / schedule_queued_jobs per
/// provider. A provider whose constructor throws (creds missing) is logged
/// and skipped. Default keeps single-cloud GCP behavior.
pub fn wc_providers() -> &'static [String] {
    &WC_PROVIDERS
}

static WC_STORAGE_BACKEND: LazyLock<String> =
    LazyLock::new(|| cfg("WC_STORAGE_BACKEND", "storage.backend", "gcs"));
static WC_AZURE_STORAGE_ACCOUNT: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AZURE_STORAGE_ACCOUNT", "storage.azure.account", ""));
static WC_AZURE_CONTAINER: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AZURE_CONTAINER", "storage.azure.container", "wisent-compute"));
static WC_S3_BUCKET: LazyLock<String> =
    LazyLock::new(|| cfg("WC_S3_BUCKET", "storage.s3.bucket", ""));
static WC_S3_REGION: LazyLock<String> = LazyLock::new(|| {
    let aws_region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    cfg("WC_S3_REGION", "storage.s3.region", &aws_region)
});
static WC_LOCAL_STORAGE_PATH: LazyLock<String> = LazyLock::new(|| {
    let default = expand_tilde("~/.stado/local-storage");
    cfg("WC_LOCAL_STORAGE_PATH", "storage.local.path", &default.to_string_lossy())
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

/// Root directory of the device-local storage backend (env
/// `WC_LOCAL_STORAGE_PATH`).
pub fn wc_local_storage_path() -> &'static str {
    WC_LOCAL_STORAGE_PATH.as_str()
}

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

/// Secret Manager secret holding the Azure billing service principal as
/// JSON {"tenant_id","client_id","client_secret", and one of
/// "billing_account"/"billing_profile" or "subscription_id"} (env
/// `WC_AZURE_BILLING_SECRET`). Absent secret is reported as an explicit
/// no_credentials status, never silently skipped.
pub fn azure_billing_secret() -> &'static str {
    AZURE_BILLING_SECRET.as_str()
}

/// In-process cache TTL for the GCS-fetched model policy (Python
/// `_MODEL_POLICY_TTL_S`).
pub const MODEL_POLICY_TTL_S: u64 = 300;

/// Co-schedule and cost-policy flags loaded from
/// `config/model_overrides.json` in the configured queue bucket.
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
        let cache = MODEL_POLICY.read().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let mut cache = MODEL_POLICY.write().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    model_policy().exclusive.iter().any(|candidate| candidate == model)
}

/// True when `model` is restricted to local execution.
pub fn is_local_only_model(model: &str) -> bool {
    model_policy().local_only.iter().any(|candidate| candidate == model)
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
    sizing.iter().next_back().map(|(_, spec)| *spec).unwrap_or(("", ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_python() {
        assert_eq!(project(), "wisent-480400");
        assert_eq!(bucket(), "stado");
        assert_eq!(region(), "us-central1");
        assert_eq!(alerts_topic(), "projects/wisent-480400/topics/stado-alerts");
        assert_eq!(regions(), ["us-central1", "europe-west4", "us-east1", "us-east4", "us-east5"]);
        assert_eq!(wc_providers(), ["gcp"]);
        assert_eq!(wc_storage_backend(), "gcs");
        assert_eq!(wc_azure_container(), "wisent-compute");
        assert_eq!(dashboard_bind(), "127.0.0.1");
        assert_eq!(dashboard_port(), 8765);
        assert_eq!(dashboard_refresh_seconds(), 10);
        assert_eq!(dashboard_agent_fresh_seconds(), 180);
        assert_eq!(azure_resource_group(), "wisent-compute");
        assert_eq!(azure_locations(), ["eastus", "westus3", "westus2", "northeurope"]);
        assert_eq!(azure_image_urn(), "microsoft-dsvm:ubuntu-hpc:2204:latest");
        assert_eq!(azure_vm_username(), "wisent");
        assert_eq!(billing_dataset(), "billing_export");
        assert_eq!(billing_table(), "gcp_billing_export_v1_017364_D3B657_F207B5");
        assert_eq!(billing_net_alert_usd(), 100.0);
        assert_eq!(azure_billing_secret(), "wisent-azure-billing-sp");
        assert!(wc_local_storage_path().ends_with(".stado/local-storage"));
    }

    #[test]
    fn zone_rotation_starts_with_primary_region() {
        let zones = zone_rotation();
        assert_eq!(&zones[..4], ["us-central1-b", "us-central1-a", "us-central1-c", "us-central1-f"]);
        assert_eq!(zones.len(), 15);
        let mt = machine_type_zones();
        assert_eq!(mt.len(), 3);
        assert!(mt["a2-ultragpu-1g"].contains(&"us-east5-a".to_string()));
    }

    #[test]
    fn lookup_instance_type_picks_smallest_fitting_tier() {
        assert_eq!(lookup_instance_type("gcp", 16), ("n1-standard-4", "nvidia-tesla-t4"));
        assert_eq!(lookup_instance_type("gcp", 17), ("g2-standard-4", "nvidia-l4"));
        assert_eq!(lookup_instance_type("azure", 24), ("Standard_NC8ads_A10_v4", "nvidia-a10"));
        // Oversized request falls back to the largest tier rather than "".
        assert_eq!(lookup_instance_type("gcp", 1000), ("a4x-highgpu-4g", "nvidia-gb200-192gb"));
        assert_eq!(lookup_instance_type("aws", 1000), ("p5.4xlarge", "nvidia-h100-80gb"));
        // Unknown provider.
        assert_eq!(lookup_instance_type("dcloud", 16), ("", ""));
    }

    #[test]
    fn model_policy_is_empty_until_gcs_fetch_is_wired() {
        assert_eq!(model_policy(), &ModelPolicy::default());
        assert!(!is_exclusive_model("any-model"));
        assert!(!is_local_only_model("any-model"));
    }
}
