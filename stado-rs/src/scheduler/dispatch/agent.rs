//! Agent-mode VM dispatch.
//!
//! Port of `stado/scheduler/dispatch/agent.py`.
//!
//! For each (accel, machine_type) bucket of queued work that isn't already
//! yielded to a local consumer, launch enough agent VMs to fill remaining
//! quota — but no more than the bucket's job count. Each VM runs
//! `wc agent --idle-shutdown`, polls the queue, packs jobs by nvidia-smi
//! VRAM, and self-terminates when no eligible queued job remains.
//!
//! Replaces the legacy 1-VM-per-job dispatch path. VRAM (read live from
//! the hardware) is the only admission constant; there is no per-VM slot
//! count.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::models::Job;
use crate::providers::{Provider, ProviderError};
use crate::queue::JobStorage;
use crate::scheduler::scheduler::{accel_hourly_rate, backoff_due, log, SchedulerError};
use crate::sizing::Sizing;

/// Per-provider agent startup-script templates, baked into the binary at
/// compile time exactly like the bundled compute-target registry
/// ([`crate::targets::load_bundled_registry`]). `data/templates/` stays
/// the single source of truth for the text; reading them back through
/// `crate::data_dir()` only ever worked on the build machine, because
/// that path is `CARGO_MANIFEST_DIR` frozen at compile time and an
/// installed `~/.stado/bin/stado` has no `data/` directory beside it.
///
/// Each launches `stado agent --kind <provider> --gpu-type <accel>
/// --idle-shutdown` after verifying and extracting the deployment-selected
/// immutable Python/model runtime bundle. Every provider receives the same
/// exact Stado release coordinates, storage/backup exports, and scoped
/// workload-secret identity.
///
/// Crate-visible so `crate::doctor` renders the preflight through the
/// identical template the dispatcher ships. A preflight that rendered its
/// own copy would prove nothing: the failure it exists to catch is a
/// placeholder no producer fills in THIS text.
pub(crate) fn bundled_template_for(provider_name: &str) -> Option<&'static str> {
    let variant =
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Execution, provider_name)?;
    match variant.adapter {
        crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Azure,
        ) => Some(include_str!(
            "../../../data/templates/startup_gpu_agent_azure.sh"
        )),
        crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Aws,
        ) => Some(include_str!(
            "../../../data/templates/startup_gpu_agent_aws.sh"
        )),
        crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Gcp
            | crate::capabilities::ExecutionAdapter::Box
            | crate::capabilities::ExecutionAdapter::Local
            | crate::capabilities::ExecutionAdapter::Vast,
        ) => Some(include_str!("../../../data/templates/startup_gpu_agent.sh")),
        _ => None,
    }
}

/// Non-secret `${KEY}` substitutions the agent templates may reference,
/// read from the process config. Deliberately NOT merged into the
/// coordinator's secrets map: these are deployment settings, not
/// credentials, and keeping the key set here — next to the templates that
/// consume it — gives a new placeholder exactly one place to be
/// registered instead of one per producer.
///
/// Every provider receives the complete primary/backup storage binding, scoped
/// agent identity, canonical provider kind, and immutable binary/runtime
/// coordinates. Secrets stay in the coordinator map: Azure consumes its raw
/// grant through protected settings, while other remote agents receive only
/// the scoped workload grant projection their root startup script materializes.
pub fn deployment_substitutions(provider_name: &str) -> BTreeMap<String, String> {
    let field = |adapter, key| {
        crate::capabilities::config_field(crate::capabilities::RuntimeFacet::Storage, adapter, key)
            .expect("storage binding is missing from the capability catalog")
    };
    let gcs_bucket = field(crate::capabilities::StorageAdapter::Gcs.id(), "bucket");
    let azure_account = field(
        crate::capabilities::StorageAdapter::AzureBlob.id(),
        "account",
    );
    let azure_container = field(
        crate::capabilities::StorageAdapter::AzureBlob.id(),
        "container",
    );
    let s3_bucket = field(crate::capabilities::StorageAdapter::S3.id(), "bucket");
    let s3_region = field(crate::capabilities::StorageAdapter::S3.id(), "region");
    let local_path = field(crate::capabilities::StorageAdapter::Local.id(), "path");
    let stado_url = field(crate::capabilities::StorageAdapter::StadoObject.id(), "url");
    let stado_token_file = field(
        crate::capabilities::StorageAdapter::StadoObject.id(),
        "token-file",
    );
    let stado_ca_file = field(
        crate::capabilities::StorageAdapter::StadoObject.id(),
        "ca-file",
    );
    let stado_namespace = field(
        crate::capabilities::StorageAdapter::StadoObject.id(),
        "namespace",
    );
    let provider_kind =
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Execution, provider_name)
            .map(|variant| variant.id)
            .unwrap_or(provider_name);
    BTreeMap::from([
        ("PROVIDER_KIND".to_string(), provider_kind.to_string()),
        (
            crate::capabilities::STORAGE_BACKEND_CONFIG.env.to_string(),
            config::wc_storage_backend().to_string(),
        ),
        (gcs_bucket.env.to_string(), config::bucket().to_string()),
        (
            azure_account.env.to_string(),
            config::wc_azure_storage_account().to_string(),
        ),
        (
            azure_container.env.to_string(),
            config::wc_azure_container().to_string(),
        ),
        (
            s3_bucket.env.to_string(),
            config::wc_s3_bucket().to_string(),
        ),
        (
            s3_region.env.to_string(),
            config::wc_s3_region().to_string(),
        ),
        (
            local_path.env.to_string(),
            config::wc_local_storage_path().to_string(),
        ),
        (
            stado_url.env.to_string(),
            config::wc_stado_storage_url().to_string(),
        ),
        (
            stado_token_file.env.to_string(),
            config::wc_stado_storage_token_file().to_string(),
        ),
        (
            stado_ca_file.env.to_string(),
            config::wc_stado_storage_ca_file().to_string(),
        ),
        (
            stado_namespace.env.to_string(),
            config::wc_stado_storage_namespace().to_string(),
        ),
        (
            crate::capabilities::STORAGE_BACKEND_CONFIG
                .backup_env
                .expect("backup storage backend environment binding is missing")
                .to_string(),
            config::wc_backup_storage_backend().to_string(),
        ),
        (
            gcs_bucket
                .backup_env
                .expect("backup bucket environment binding is missing")
                .to_string(),
            config::wc_backup_bucket().to_string(),
        ),
        (
            azure_account
                .backup_env
                .expect("backup Azure account environment binding is missing")
                .to_string(),
            config::wc_backup_azure_storage_account().to_string(),
        ),
        (
            azure_container
                .backup_env
                .expect("backup Azure container environment binding is missing")
                .to_string(),
            config::wc_backup_azure_container().to_string(),
        ),
        (
            s3_region
                .backup_env
                .expect("backup S3 region environment binding is missing")
                .to_string(),
            config::wc_backup_s3_region().to_string(),
        ),
        (
            local_path
                .backup_env
                .expect("backup local path environment binding is missing")
                .to_string(),
            config::wc_backup_local_storage_path().to_string(),
        ),
        (
            "WC_AGENT_SKARBIEC_URL".to_string(),
            config::agent_skarbiec_url().to_string(),
        ),
        (
            "WC_AGENT_SKARBIEC_CONSUMER".to_string(),
            config::agent_skarbiec_consumer().to_string(),
        ),
        (
            "WC_AGENT_SKARBIEC_ITEMS".to_string(),
            config::agent_skarbiec_items().join(","),
        ),
        (
            "WC_AGENT_SKARBIEC_SECRET_FIELDS".to_string(),
            config::agent_skarbiec_secret_fields().join(","),
        ),
        (
            "STADO_RELEASE_API_URL".to_string(),
            config::stado_release_api_url(),
        ),
        (
            "STADO_RELEASE_VERSION".to_string(),
            config::stado_release_version(),
        ),
        (
            "STADO_RELEASE_PLATFORM".to_string(),
            config::stado_release_platform(),
        ),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_URI".to_string(),
            config::stado_agent_runtime_bundle_uri(),
        ),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_SHA256".to_string(),
            config::stado_agent_runtime_bundle_sha256(),
        ),
        ("AWS_REGION".to_string(), config::aws_region().to_string()),
    ])
}

const REQUIRED_AGENT_EXPORTS: &[&str] = &[
    "WC_STORAGE_BACKEND",
    "WC_BUCKET",
    "WC_AZURE_STORAGE_ACCOUNT",
    "WC_AZURE_CONTAINER",
    "WC_S3_BUCKET",
    "WC_S3_REGION",
    "WC_LOCAL_STORAGE_PATH",
    "WC_STADO_STORAGE_URL",
    "WC_STADO_STORAGE_TOKEN_FILE",
    "WC_STADO_STORAGE_CA_FILE",
    "WC_STADO_STORAGE_NAMESPACE",
    "WC_BACKUP_STORAGE_BACKEND",
    "WC_BACKUP_BUCKET",
    "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
    "WC_BACKUP_AZURE_CONTAINER",
    "WC_BACKUP_S3_REGION",
    "WC_BACKUP_LOCAL_STORAGE_PATH",
    "WC_AGENT_SKARBIEC_URL",
    "WC_AGENT_SKARBIEC_CONSUMER",
    "WC_AGENT_SKARBIEC_ITEMS",
    "WC_AGENT_SKARBIEC_SECRET_FIELDS",
];

fn require_deployment_setting(
    deployment: &BTreeMap<String, String>,
    key: &'static str,
    config_key: &'static str,
) -> Result<(), SchedulerError> {
    if deployment
        .get(key)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    Err(SchedulerError::MissingStartupSetting {
        key: key.to_string(),
        env: key,
        config_key,
    })
}

fn validate_storage_settings(deployment: &BTreeMap<String, String>) -> Result<(), SchedulerError> {
    require_deployment_setting(deployment, "WC_STORAGE_BACKEND", "storage.backend")?;
    match deployment.get("WC_STORAGE_BACKEND").map(String::as_str) {
        Some("gcs") => require_deployment_setting(deployment, "WC_BUCKET", "storage.gcs.bucket")?,
        Some("azure") => {
            require_deployment_setting(
                deployment,
                "WC_AZURE_STORAGE_ACCOUNT",
                "storage.azure.account",
            )?;
            require_deployment_setting(
                deployment,
                "WC_AZURE_CONTAINER",
                "storage.azure.container",
            )?;
        }
        Some("s3") => {
            require_deployment_setting(deployment, "WC_S3_BUCKET", "storage.s3.bucket")?;
        }
        Some("local") => {
            require_deployment_setting(deployment, "WC_LOCAL_STORAGE_PATH", "storage.local.path")?;
        }
        Some("stado") => {
            require_deployment_setting(deployment, "WC_STADO_STORAGE_URL", "storage.stado.url")?;
            require_deployment_setting(
                deployment,
                "WC_STADO_STORAGE_TOKEN_FILE",
                "storage.stado.token_file",
            )?;
            require_deployment_setting(
                deployment,
                "WC_STADO_STORAGE_NAMESPACE",
                "storage.stado.namespace",
            )?;
        }
        Some(_) | None => {
            return Err(SchedulerError::InvalidStartupSetting {
                key: "WC_STORAGE_BACKEND".to_string(),
                env: "WC_STORAGE_BACKEND",
                config_key: "storage.backend",
                reason: "expected gcs, azure, s3, stado, or local",
            });
        }
    }
    match deployment
        .get("WC_BACKUP_STORAGE_BACKEND")
        .map(String::as_str)
    {
        Some("gcs") => {
            require_deployment_setting(deployment, "WC_BACKUP_BUCKET", "storage.backup.bucket")?
        }
        Some("azure") => {
            require_deployment_setting(
                deployment,
                "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
                "storage.backup.azure.account",
            )?;
            require_deployment_setting(
                deployment,
                "WC_BACKUP_AZURE_CONTAINER",
                "storage.backup.azure.container",
            )?;
        }
        Some("s3") => {
            require_deployment_setting(deployment, "WC_BACKUP_BUCKET", "storage.backup.bucket")?
        }
        Some("local") => require_deployment_setting(
            deployment,
            "WC_BACKUP_LOCAL_STORAGE_PATH",
            "storage.backup.local.path",
        )?,
        Some("") | None => {}
        Some(_) => {
            return Err(SchedulerError::InvalidStartupSetting {
                key: "WC_BACKUP_STORAGE_BACKEND".to_string(),
                env: "WC_BACKUP_STORAGE_BACKEND",
                config_key: "storage.backup.backend",
                reason: "expected empty, gcs, azure, s3, or local",
            });
        }
    }
    Ok(())
}

/// Validate every live template's source-level export contract before plain
/// substitution can hide an omitted setting.
pub fn render_agent_startup_script(
    provider_name: &str,
    template: &str,
    accel: &str,
    secrets: &BTreeMap<String, String>,
    deployment: &BTreeMap<String, String>,
) -> Result<String, SchedulerError> {
    for key in REQUIRED_AGENT_EXPORTS {
        let marker = format!("export {key}=\"${{{key}}}\"");
        if !template.contains(&marker) {
            return Err(SchedulerError::MissingStartupExport {
                provider: provider_name.to_string(),
                key: (*key).to_string(),
            });
        }
    }
    for (key, config_key) in [
        ("PROVIDER_KIND", "providers"),
        ("STADO_RELEASE_API_URL", "release.api_url"),
        ("STADO_RELEASE_VERSION", "release.version"),
        ("STADO_RELEASE_PLATFORM", "release.platform"),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_URI",
            "release.agent_runtime_bundle_uri",
        ),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_SHA256",
            "release.agent_runtime_bundle_sha256",
        ),
        ("WC_AGENT_SKARBIEC_URL", "agent.skarbiec.url"),
        ("WC_AGENT_SKARBIEC_CONSUMER", "agent.skarbiec.consumer"),
    ] {
        require_deployment_setting(deployment, key, config_key)?;
        if !template.contains(format!("${{{key}}}").as_str()) {
            return Err(SchedulerError::MissingStartupExport {
                provider: provider_name.to_string(),
                key: key.to_string(),
            });
        }
    }
    let release_api = deployment
        .get("STADO_RELEASE_API_URL")
        .map(String::as_str)
        .unwrap_or_default();
    if !release_api.starts_with("https://") {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_RELEASE_API_URL".to_string(),
            env: "STADO_RELEASE_API_URL",
            config_key: "release.api_url",
            reason: "expected an HTTPS Stado release API origin",
        });
    }
    for (key, config_key) in [
        ("STADO_RELEASE_VERSION", "release.version"),
        ("STADO_RELEASE_PLATFORM", "release.platform"),
    ] {
        let value = deployment.get(key).map(String::as_str).unwrap_or_default();
        let canonical = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && !matches!(value, "latest" | "stable" | "main" | "master");
        if !canonical {
            return Err(SchedulerError::InvalidStartupSetting {
                key: key.to_string(),
                env: key,
                config_key,
                reason: "expected an exact non-channel release coordinate",
            });
        }
    }
    let runtime_uri = deployment
        .get("STADO_AGENT_RUNTIME_BUNDLE_URI")
        .map(String::as_str)
        .unwrap_or_default();
    let mut runtime_segments = runtime_uri
        .strip_prefix("stado://releases/")
        .unwrap_or_default()
        .split('/');
    let product = runtime_segments.next().unwrap_or_default();
    let version = runtime_segments.next().unwrap_or_default();
    let platform = runtime_segments.next().unwrap_or_default();
    let object = runtime_segments.next().unwrap_or_default();
    let canonical_runtime_uri = [product, version, platform, object].iter().all(|segment| {
        !segment.is_empty()
            && *segment != "."
            && *segment != ".."
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    }) && runtime_segments.next().is_none()
        && !matches!(version, "latest" | "stable" | "main" | "master");
    if !canonical_runtime_uri {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_AGENT_RUNTIME_BUNDLE_URI".to_string(),
            env: "STADO_AGENT_RUNTIME_BUNDLE_URI",
            config_key: "release.agent_runtime_bundle_uri",
            reason: "expected canonical stado://releases/<product>/<version>/<platform>/<object> with an exact non-channel version",
        });
    }
    let runtime_sha = deployment
        .get("STADO_AGENT_RUNTIME_BUNDLE_SHA256")
        .map(String::as_str)
        .unwrap_or_default();
    if runtime_sha.len() != "64".parse::<usize>().expect("static SHA-256 hex length")
        || !runtime_sha.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_AGENT_RUNTIME_BUNDLE_SHA256".to_string(),
            env: "STADO_AGENT_RUNTIME_BUNDLE_SHA256",
            config_key: "release.agent_runtime_bundle_sha256",
            reason: "expected one exact SHA-256 hex digest",
        });
    }
    validate_storage_settings(deployment)?;
    let adapter = crate::capabilities::execution_adapter(provider_name);
    if adapter == Some(crate::capabilities::ExecutionAdapter::Aws)
        && !template.contains("export AWS_REGION=\"${AWS_REGION}\"")
    {
        return Err(SchedulerError::MissingStartupExport {
            provider: provider_name.to_string(),
            key: "AWS_REGION".to_string(),
        });
    }
    let azure = adapter == Some(crate::capabilities::ExecutionAdapter::Azure);
    if !azure {
        let key = crate::coordinator::AGENT_WORKLOAD_GRANT_B64;
        if !template.contains(format!("${{{key}}}").as_str()) {
            return Err(SchedulerError::MissingStartupExport {
                provider: provider_name.to_string(),
                key: key.to_string(),
            });
        }
        if secrets.get(key).is_none_or(|value| value.is_empty()) {
            return Err(SchedulerError::MissingStartupSetting {
                key: key.to_string(),
                env: "WC_AGENT_SKARBIEC_TOKEN_FILE",
                config_key: "agent.skarbiec.token_file",
            });
        }
    }
    render_startup_script(template, accel, secrets, deployment)
}

/// Substitute `${ACCEL_TYPE}`, every `${KEY}` secret and every non-secret
/// deployment key into the template. Python does plain str.replace per
/// key, so only keys present in the maps are substituted — the `${...}`
/// forms the templates keep for the VM's own shell are left alone (see
/// [`unresolved_placeholder`]). Secrets are NEVER logged: the rendered
/// script goes straight to create_instance, only the instance ref / accel
/// / machine reach the log lines, and the error below carries a
/// placeholder name, never a value.
///
/// Secrets win over deployment config on a duplicate key, so an operator
/// export still overrides a config-file default.
///
/// Errors when a dispatcher-owned placeholder survives rendering, so a key
/// the templates need but no producer supplies can never again reach a VM
/// and kill its boot on `set -u`.
pub fn render_startup_script(
    template: &str,
    accel: &str,
    secrets: &BTreeMap<String, String>,
    deployment: &BTreeMap<String, String>,
) -> Result<String, SchedulerError> {
    let mut script = template.replace("${ACCEL_TYPE}", accel);
    for (key, val) in secrets
        .iter()
        .filter(|(key, _)| key.as_str() != crate::coordinator::AZURE_AGENT_PROTECTED_GRANT)
        .chain(deployment.iter())
    {
        let needle = format!("${{{key}}}");
        if script.contains(needle.as_str()) {
            script = script.replace(needle.as_str(), val);
        }
    }
    match unresolved_placeholder(&script) {
        Some(key) => Err(SchedulerError::UnresolvedPlaceholder {
            key: key.to_string(),
        }),
        None => Ok(script),
    }
}

/// First dispatcher-owned `${NAME}` still standing in a rendered script.
/// Bare SCREAMING_SNAKE placeholders belong to dispatch; shell locals,
/// underscore-prefixed names, and parameter-expansion operators remain for
/// the VM's own shell.
fn unresolved_placeholder(script: &str) -> Option<&str> {
    let mut parts = script.split("${");
    parts.next();
    parts.find_map(|part| {
        let (name, _) = part.split_once('}')?;
        let dispatcher_owned = name.starts_with(|c: char| c.is_ascii_uppercase())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        dispatcher_owned.then_some(name)
    })
}

/// Inputs shared by the caller's per-tick budgets; `available` and
/// `accel_dispatched` are mutated in place so the caller's books stay
/// consistent with cloud reality (Python passes dicts by reference).
pub struct AgentDispatchInputs<'a> {
    pub queued: Vec<Job>,
    pub yield_targets: HashMap<String, String>,
    pub available: &'a mut BTreeMap<String, i64>,
    pub accel_dispatched: &'a mut BTreeMap<String, i64>,
    pub per_accel_share: i64,
    pub per_tick_cap: i64,
    pub scheduled_so_far: i64,
}

/// The bucketing half of Python `dispatch_agent_vms`, split out for
/// tests. Bucket key is (accel, mt); bucket order is first-seen
/// (Python dict insertion order), which decides which buckets win the
/// per-tick cap when the queue is deep.
///
/// Bucket key is (accel, mt). Default: derive from current GPU_SIZING
/// via lookup_instance_type — protects against stale job-level machine
/// specs (e.g. a2-highgpu-2g + nvidia-tesla-a100 for 60GB jobs that GCP
/// rejects with 'Invalid accelerator specs for accelerator optimized
/// instances'). Override: caller-pinned job.machine_type wins so users
/// who need a specific host (g2-standard-8 for 32 GB RAM on an L4 job)
/// don't get silently downgraded back to the default-tier g2-standard-4
/// (16 GB RAM, repeated host-OOM source for diffusion training).
pub(crate) async fn bucket_jobs(
    queued: &[Job],
    yield_targets: &HashMap<String, String>,
    provider_name: &str,
    sizing: &Sizing,
    store: &JobStorage,
    now_utc: DateTime<Utc>,
) -> Result<Vec<((String, String), Vec<Job>)>, SchedulerError> {
    let mut buckets: Vec<((String, String), Vec<Job>)> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for j in queued {
        if j.pin_to_provider && j.provider != provider_name {
            continue;
        }
        if yield_targets.contains_key(&j.job_id) {
            continue;
        }
        if !backoff_due(j, now_utc) {
            continue;
        }
        let mut gpu_mem = j.gpu_mem_gb;
        if gpu_mem <= 0 {
            // Unmeasured on the queue blob — normalize_queue_sizing forces
            // gpu_mem_gb=0 whenever observed_vram_gb(model) is None at
            // sizing time (sizing/__init__.py docstring). Previously this
            // branch was a hard `continue`, which combined with the
            // always-write-0 behaviour to lock the entire
            // unmeasured-model queue out of the autoscaler (199
            // gpt-oss-20b jobs stuck at gpu_mem_gb=0 observed live
            // 2026-05-20). Recover by:
            //   1. Re-checking observed_vram_gb (a sibling job of the
            //      same model may have just completed and populated the
            //      map),
            //   2. Falling back to smallest_live_vram() — the documented
            //      start size for unmeasured models (see
            //      escalate_on_oom). If the job overflows that tier,
            //      escalate_on_oom climbs to next_live_vram on requeue.
            // If neither yields a number, the fleet has no live GPU
            // broadcasting at all and the job is genuinely unschedulable
            // this tick; defer to the next.
            let model = crate::sizing::model_of(&j.command);
            let peak = if model.is_empty() {
                None
            } else {
                sizing.observed_vram_gb(store, &model).await?
            };
            if let Some(peak) = peak {
                if peak > 0 {
                    gpu_mem = peak;
                }
            }
            if gpu_mem <= 0 {
                let Some(live_small) = sizing.smallest_live_vram(store).await? else {
                    continue;
                };
                gpu_mem = live_small;
            }
        }
        let (default_mt, default_accel) = config::lookup_instance_type(provider_name, gpu_mem);
        if default_accel.is_empty() || default_mt.is_empty() {
            continue;
        }
        // Caller-pinned overrides — fall back to catalog if either is
        // empty.
        let mt = {
            let pinned = j.machine_type.trim();
            if pinned.is_empty() {
                default_mt
            } else {
                pinned
            }
        };
        let accel = {
            let pinned = j.gpu_type.trim();
            if pinned.is_empty() {
                default_accel
            } else {
                pinned
            }
        };
        let cap = j.max_cost_per_hour_usd;
        if cap > 0.0 && !accel.is_empty() {
            let rate = accel_hourly_rate(accel, j.preemptible);
            if rate > 0.0 && rate > cap {
                continue;
            }
        }
        let key = (accel.to_string(), mt.to_string());
        match index.get(&key) {
            Some(&idx) => buckets[idx].1.push(j.clone()),
            None => {
                index.insert(key.clone(), buckets.len());
                buckets.push((key, vec![j.clone()]));
            }
        }
    }
    Ok(buckets)
}

/// Group queued jobs by (accel, machine_type) and launch agent VMs.
/// Returns the number of agent VMs created. Python `dispatch_agent_vms`.
pub async fn dispatch_agent_vms(
    inputs: AgentDispatchInputs<'_>,
    store: &JobStorage,
    sizing: &Sizing,
    provider: &dyn Provider,
    provider_name: &str,
    secrets: &BTreeMap<String, String>,
    now_utc: DateTime<Utc>,
) -> Result<i64, SchedulerError> {
    let template = bundled_template_for(provider_name).ok_or_else(|| {
        crate::providers::ProviderError::Value(format!(
            "provider {provider_name:?} has no execution template"
        ))
    })?;
    let deployment = deployment_substitutions(provider_name);
    dispatch_agent_vms_with_template(
        inputs,
        template,
        store,
        sizing,
        provider,
        provider_name,
        secrets,
        &deployment,
        now_utc,
    )
    .await
}

/// [`dispatch_agent_vms`] with the startup-script template and deployment
/// settings injected so tests remain independent of ambient configuration.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_agent_vms_with_template(
    inputs: AgentDispatchInputs<'_>,
    template: &str,
    store: &JobStorage,
    sizing: &Sizing,
    provider: &dyn Provider,
    provider_name: &str,
    secrets: &BTreeMap<String, String>,
    deployment: &BTreeMap<String, String>,
    now_utc: DateTime<Utc>,
) -> Result<i64, SchedulerError> {
    let AgentDispatchInputs {
        queued,
        yield_targets,
        available,
        accel_dispatched,
        per_accel_share,
        per_tick_cap,
        scheduled_so_far,
    } = inputs;
    let buckets = bucket_jobs(
        &queued,
        &yield_targets,
        provider_name,
        sizing,
        store,
        now_utc,
    )
    .await?;

    let protected_agent_grant = if matches!(
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Execution, provider_name,)
            .map(|variant| variant.adapter),
        Some(crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Azure
        ))
    ) {
        Some(
            secrets
                .get(crate::coordinator::AZURE_AGENT_PROTECTED_GRANT)
                .filter(|grant| !grant.is_empty())
                .map(String::as_str)
                .ok_or_else(|| {
                    ProviderError::Value(
                        "Azure agent dispatch requires a dedicated grant for protected-settings delivery"
                            .to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    let tick_tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut created: i64 = 0;
    let mut scheduled = scheduled_so_far;
    // Time budget: each create_instance can spend ~10s/zone × 7+ zones
    // for first-encounter stockouts, plus a full retry on the larger tier
    // in the escalation branch. With n_to_dispatch=2-3 per bucket, the
    // autoscaler can easily eat 300+ seconds — confirmed live 03:39Z
    // 2026-05-15 tick 504'd at 540s. Bail out after 120s in the
    // dispatcher and let the next tick try again (caches will be warm).
    const DISPATCH_BUDGET_S: u64 = 120;
    let start = Instant::now();
    'buckets: for ((accel, mt), jobs) in &buckets {
        if scheduled >= per_tick_cap {
            break;
        }
        if start.elapsed().as_secs() > DISPATCH_BUDGET_S {
            log(&format!(
                "dispatch budget exhausted after {scheduled} scheduled; deferring remaining buckets to next tick"
            ));
            break;
        }
        let quota_left = available.get(accel).copied().unwrap_or(0);
        if quota_left <= 0 {
            log(&format!(
                "Skip bucket accel={accel} machine={mt}: 0 quota slots"
            ));
            continue;
        }
        let share_left = per_accel_share - accel_dispatched.get(accel).copied().unwrap_or(0);
        if share_left <= 0 {
            continue;
        }
        let n_to_dispatch = (jobs.len() as i64)
            .min(quota_left)
            .min(share_left)
            .min(per_tick_cap - scheduled);
        let biggest = jobs
            .iter()
            .max_by_key(|j| j.gpu_mem_gb)
            .expect("bucket is non-empty");
        // No-preemptible policy: per user instruction (2026-05-06), this
        // codebase is NOT to dispatch Spot/preemptible VMs even when the
        // job's `preemptible` field is True. Repeated Spot reclaims of
        // A100-80 capacity in us-central1 caused 8 cloud-agent VMs to be
        // deleted under instance_termination_action=DELETE in a single
        // 3-second window (22:21:10-13Z), forcing requeues that burned
        // restart-budget on misclassified jobs (since fixed in 0.4.55,
        // but the underlying preemption noise persists). Override the
        // job-level flag and force every dispatch to STANDARD.
        let preemptible_for_call = false;
        // Render once per bucket: the script depends only on the template,
        // the bucket's accel and the substitution maps, never on the
        // instance index. A template whose placeholders cannot all be
        // filled stops dispatch here — before any create_instance — rather
        // than booting VMs that `set -u` kills on their first export.
        let script = match render_agent_startup_script(
            provider_name,
            template,
            accel,
            secrets,
            deployment,
        ) {
            Ok(script) => script,
            Err(exc) => {
                log(&format!(
                    "REFUSING to dispatch agent VMs for accel={accel} machine={mt}: {exc}. \
                     No instance was created; fix the coordinator env/config and the next \
                     tick retries."
                ));
                break 'buckets;
            }
        };
        if protected_agent_grant.is_some_and(|grant| script.contains(grant)) {
            return Err(ProviderError::Value(
                "refusing Azure dispatch because the protected agent grant reached customData"
                    .to_string(),
            )
            .into());
        }
        for i in 0..n_to_dispatch {
            if start.elapsed().as_secs() > DISPATCH_BUDGET_S {
                log(&format!(
                    "dispatch budget exhausted mid-bucket {accel}; deferring"
                ));
                break 'buckets;
            }
            let instance_name = format!(
                "{}-agent-{}-{tick_tag}-{i}",
                config::INSTANCE_PREFIX,
                accel.rsplit('-').next().unwrap_or(accel)
            );
            let mut effective_accel = accel.clone();
            let mut ref_opt = provider
                .create_agent_instance(
                    &instance_name,
                    mt,
                    accel,
                    biggest.boot_disk_gb,
                    &biggest.image,
                    &biggest.image_project,
                    &script,
                    preemptible_for_call,
                    protected_agent_grant,
                )
                .await?;
            if ref_opt.is_none() {
                log(&format!(
                    "Agent VM create failed accel={accel} machine={mt}"
                ));
                // Stockout-aware escalation: when create_instance returns
                // None (zone STOCKOUTs across all configured zones for
                // this accel), try the next-larger tier from GPU_SIZING.
                // The job is larger than needed but routes around the
                // capacity shortage. The same VM tier returns on next
                // tick if the operator hasn't manually re-routed.
                let pmem = biggest.gpu_mem_gb;
                let mut escalated = false;
                if let Some(sizing_map) = GPU_SIZING.get(provider_name) {
                    for (next_mem, (next_mt, next_accel)) in sizing_map.range(pmem + 1..) {
                        if next_accel == &accel.as_str() && next_mt == &mt.as_str() {
                            continue;
                        }
                        if available.get(*next_accel).copied().unwrap_or(0) <= 0 {
                            continue;
                        }
                        log(&format!(
                            "escalating {accel}/{mt} -> {next_accel}/{next_mt} \
                             (stockout on {accel}, next tier mem={next_mem})"
                        ));
                        ref_opt = provider
                            .create_agent_instance(
                                &instance_name,
                                next_mt,
                                next_accel,
                                biggest.boot_disk_gb,
                                &biggest.image,
                                &biggest.image_project,
                                &script,
                                preemptible_for_call,
                                protected_agent_grant,
                            )
                            .await?;
                        if ref_opt.is_some() {
                            effective_accel = next_accel.to_string();
                            escalated = true;
                            break;
                        }
                    }
                }
                if !escalated {
                    continue;
                }
            }
            let instance_ref = ref_opt.expect("escalated or initial create returned a ref");
            *available.entry(effective_accel.clone()).or_insert(0) -= 1;
            *accel_dispatched.entry(effective_accel.clone()).or_insert(0) += 1;
            scheduled += 1;
            created += 1;
            log(&format!(
                "Dispatched agent VM {instance_ref} accel={effective_accel} machine={mt} \
                 preemptible={preemptible_for_call}"
            ));
            if scheduled >= per_tick_cap {
                break;
            }
        }
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::{Arc, Mutex};

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    #[test]
    fn stado_storage_settings_are_accepted_for_remote_agents() {
        let mut deployment = BTreeMap::from([
            ("WC_STORAGE_BACKEND".to_string(), "stado".to_string()),
            (
                "WC_STADO_STORAGE_URL".to_string(),
                "https://queue.example.test".to_string(),
            ),
            (
                "WC_STADO_STORAGE_TOKEN_FILE".to_string(),
                "/run/stado-agent-credentials/object-token".to_string(),
            ),
            (
                "WC_STADO_STORAGE_NAMESPACE".to_string(),
                "fleet".to_string(),
            ),
            ("WC_BACKUP_STORAGE_BACKEND".to_string(), String::new()),
        ]);
        validate_storage_settings(&deployment).expect("stado storage settings");

        deployment.remove("WC_STADO_STORAGE_TOKEN_FILE");
        assert!(matches!(
            validate_storage_settings(&deployment),
            Err(SchedulerError::MissingStartupSetting {
                env: "WC_STADO_STORAGE_TOKEN_FILE",
                ..
            })
        ));
    }

    fn job(job_id: &str, gpu_mem_gb: i64, gpu_type: &str, machine_type: &str) -> Job {
        let mut job = Job::new(job_id, "run --model org/m");
        job.gpu_mem_gb = gpu_mem_gb;
        job.gpu_type = gpu_type.into();
        job.machine_type = machine_type.into();
        job
    }

    /// Records create_instance calls; the per-call results drive
    /// stockout/escalation tests.
    #[derive(Default)]
    struct FakeProvider {
        calls: Mutex<Vec<(String, String, String, bool)>>,
        results: Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl Provider for FakeProvider {
        async fn create_instance(
            &self,
            name: &str,
            machine_type: &str,
            accel_type: &str,
            _d: i64,
            _i: &str,
            _p: &str,
            _s: &str,
            preemptible: bool,
        ) -> Result<Option<String>, crate::providers::ProviderError> {
            self.calls.lock().unwrap().push((
                name.to_string(),
                machine_type.to_string(),
                accel_type.to_string(),
                preemptible,
            ));
            let mut results = self.results.lock().unwrap();
            if results.is_empty() {
                return Ok(Some(format!("{name}@zone")));
            }
            Ok(results.remove(0))
        }
        async fn delete_instance(&self, _r: &str) -> Result<(), crate::providers::ProviderError> {
            Ok(())
        }
        async fn instance_exists(&self, _r: &str) -> Result<bool, crate::providers::ProviderError> {
            Ok(false)
        }
        async fn list_running_instances(
            &self,
        ) -> Result<BTreeMap<String, i64>, crate::providers::ProviderError> {
            Ok(BTreeMap::new())
        }
    }

    #[test]
    fn render_startup_script_substitutes_secrets_without_placeholder_leakage() {
        let template = "#!/bin/bash\nexport HF_TOKEN=${HF_TOKEN}\nexport WANDB=${WANDB_API_KEY}\n\
                        echo ${ACCEL_TYPE} $HOME ${NOT_A_SECRET:-}\n";
        let secrets = BTreeMap::from([
            ("HF_TOKEN".to_string(), "hf_zzz".to_string()),
            ("WANDB_API_KEY".to_string(), "wb-secret".to_string()),
        ]);
        let script =
            render_startup_script(template, "nvidia-l4", &secrets, &BTreeMap::new()).unwrap();
        assert!(script.contains("export HF_TOKEN=hf_zzz"), "{script}");
        assert!(script.contains("export WANDB=wb-secret"), "{script}");
        assert!(script.contains("echo nvidia-l4"), "{script}");
        // No placeholder leakage for substituted keys; untouched shell
        // expansions survive for the VM's shell.
        assert!(!script.contains("${HF_TOKEN}"), "{script}");
        assert!(!script.contains("${ACCEL_TYPE}"), "{script}");
        assert!(script.contains("${NOT_A_SECRET:-}"), "{script}");
        assert!(script.contains("$HOME"), "{script}");
    }

    #[tokio::test]
    async fn bucketing_groups_by_accel_and_machine_type_first_seen_order() {
        let (_dir, store) = store();
        let now = Utc::now();
        let yields = HashMap::from([("yielded".to_string(), "local-1".to_string())]);
        let queued = vec![
            job("t4-1", 16, "", ""),
            job("l4-1", 24, "", ""),
            job("t4-2", 16, "", ""),
            job("yielded", 16, "", ""),
            job("l4-pinned", 24, "nvidia-l4", "g2-standard-8"),
        ];
        let buckets = bucket_jobs(
            &queued,
            &yields,
            "gcp",
            crate::sizing::global(),
            &store,
            now,
        )
        .await
        .unwrap();
        let keys: Vec<&(String, String)> = buckets.iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            vec![
                &("nvidia-tesla-t4".to_string(), "n1-standard-4".to_string()),
                &("nvidia-l4".to_string(), "g2-standard-4".to_string()),
                &("nvidia-l4".to_string(), "g2-standard-8".to_string()),
            ]
        );
        assert_eq!(buckets[0].1.len(), 2);
        assert_eq!(buckets[1].1.len(), 1);
        // The caller-pinned machine_type override is not downgraded.
        assert_eq!(buckets[2].1[0].job_id, "l4-pinned");
    }

    #[tokio::test]
    async fn bucketing_skips_backoff_pinned_foreign_and_over_cap_jobs() {
        let (_dir, store) = store();
        let now = Utc::now();
        let mut wedged = job("wedged", 16, "", "");
        wedged.dispatch_attempts = 3;
        wedged.last_dispatch_attempt = Some(now.to_rfc3339()); // 15m window, just attempted
        let mut foreign = job("foreign", 16, "", "");
        foreign.pin_to_provider = true;
        foreign.provider = "azure".into();
        let mut capped = job("capped", 16, "", "");
        capped.max_cost_per_hour_usd = 0.10; // t4 on-demand rate 0.35 > cap
        let ok = job("ok", 16, "", "");
        let buckets = bucket_jobs(
            &[wedged, foreign, capped, ok],
            &HashMap::new(),
            "gcp",
            crate::sizing::global(),
            &store,
            now,
        )
        .await
        .unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].1.len(), 1);
        assert_eq!(buckets[0].1[0].job_id, "ok");
    }

    fn dispatch_inputs<'a>(
        queued: Vec<Job>,
        available: &'a mut BTreeMap<String, i64>,
        dispatched: &'a mut BTreeMap<String, i64>,
    ) -> AgentDispatchInputs<'a> {
        AgentDispatchInputs {
            queued,
            yield_targets: HashMap::new(),
            available,
            accel_dispatched: dispatched,
            per_accel_share: 25,
            per_tick_cap: 25,
            scheduled_so_far: 0,
        }
    }

    const TEMPLATE: &str = include_str!("../../../data/templates/startup_gpu_agent.sh");

    fn dispatch_secrets() -> BTreeMap<String, String> {
        BTreeMap::from([(
            crate::coordinator::AGENT_WORKLOAD_GRANT_B64.to_string(),
            "dGVzdC13b3JrbG9hZC1ncmFudA==".to_string(),
        )])
    }

    fn dispatch_deployment() -> BTreeMap<String, String> {
        let mut deployment = BTreeMap::from([
            ("PROVIDER_KIND".to_string(), "gcp".to_string()),
            (
                "STADO_RELEASE_API_URL".to_string(),
                "https://release.test".to_string(),
            ),
            ("STADO_RELEASE_VERSION".to_string(), "1.2.3".to_string()),
            (
                "STADO_RELEASE_PLATFORM".to_string(),
                "linux-amd64".to_string(),
            ),
            (
                "STADO_AGENT_RUNTIME_BUNDLE_URI".to_string(),
                "stado://releases/stado/1.2.3/linux-amd64/stado-agent-runtime.tar.gz".to_string(),
            ),
            (
                "STADO_AGENT_RUNTIME_BUNDLE_SHA256".to_string(),
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            ),
            (
                "WC_AGENT_SKARBIEC_URL".to_string(),
                "https://skarbiec.test".to_string(),
            ),
            (
                "WC_AGENT_SKARBIEC_CONSUMER".to_string(),
                "stado-agent".to_string(),
            ),
            ("WC_STORAGE_BACKEND".to_string(), "gcs".to_string()),
            ("WC_BUCKET".to_string(), "test-bucket".to_string()),
            ("WC_BACKUP_STORAGE_BACKEND".to_string(), String::new()),
        ]);
        for key in REQUIRED_AGENT_EXPORTS {
            deployment.entry((*key).to_string()).or_default();
        }
        deployment.insert("AWS_REGION".to_string(), String::new());
        deployment
    }

    #[tokio::test]
    async fn dispatch_creates_min_of_jobs_quota_and_cap_per_bucket() {
        let (_dir, store) = store();
        let provider = FakeProvider::default();
        let mut secrets = dispatch_secrets();
        secrets.insert("HF_TOKEN".to_string(), "hf_zzz".to_string());
        let deployment = dispatch_deployment();
        let mut available = BTreeMap::from([("nvidia-tesla-t4".to_string(), 2)]);
        let mut dispatched = BTreeMap::new();
        let queued = vec![
            job("a", 16, "", ""),
            job("b", 16, "", ""),
            job("c", 16, "", ""),
        ];
        let created = dispatch_agent_vms_with_template(
            dispatch_inputs(queued, &mut available, &mut dispatched),
            TEMPLATE,
            &store,
            crate::sizing::global(),
            &provider,
            "gcp",
            &secrets,
            &deployment,
            Utc::now(),
        )
        .await
        .unwrap();
        // 3 jobs but only 2 quota slots.
        assert_eq!(created, 2);
        assert_eq!(available["nvidia-tesla-t4"], 0);
        assert_eq!(dispatched["nvidia-tesla-t4"], 2);
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        // No-preemptible policy: every dispatch forces STANDARD even when
        // jobs ask for Spot.
        assert!(calls.iter().all(|(_, _, _, preemptible)| !preemptible));
        assert_eq!(calls[0].1, "n1-standard-4");
        assert_eq!(calls[0].2, "nvidia-tesla-t4");
    }

    #[tokio::test]
    async fn stockout_escalates_to_next_larger_gpu_sizing_tier() {
        let (_dir, store) = store();
        let provider = FakeProvider::default();
        // First create (t4) stockouts -> None; the escalated l4 create
        // succeeds.
        *provider.results.lock().unwrap() = vec![None, Some("wisent-agent-l4-t@z".to_string())];
        let secrets = dispatch_secrets();
        let deployment = dispatch_deployment();
        let mut available = BTreeMap::from([
            ("nvidia-tesla-t4".to_string(), 1),
            ("nvidia-l4".to_string(), 1),
        ]);
        let mut dispatched = BTreeMap::new();
        let queued = vec![job("a", 16, "", "")];
        let created = dispatch_agent_vms_with_template(
            dispatch_inputs(queued, &mut available, &mut dispatched),
            TEMPLATE,
            &store,
            crate::sizing::global(),
            &provider,
            "gcp",
            &secrets,
            &deployment,
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(created, 1);
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(calls[0].1, "n1-standard-4"); // original tier
        assert_eq!(calls[1].1, "g2-standard-4"); // escalated tier (24GB l4)
        assert_eq!(calls[1].2, "nvidia-l4");
        // The t4 quota is untouched; the escalation consumed l4 quota.
        assert_eq!(available["nvidia-tesla-t4"], 1);
        assert_eq!(available["nvidia-l4"], 0);
        assert_eq!(dispatched["nvidia-l4"], 1);
    }

    #[tokio::test]
    async fn stockout_without_larger_tier_quota_does_not_escalate() {
        let (_dir, store) = store();
        let provider = FakeProvider::default();
        *provider.results.lock().unwrap() = vec![None];
        let secrets = dispatch_secrets();
        let deployment = dispatch_deployment();
        // No l4 quota -> escalation is skipped entirely.
        let mut available = BTreeMap::from([("nvidia-tesla-t4".to_string(), 1)]);
        let mut dispatched = BTreeMap::new();
        let created = dispatch_agent_vms_with_template(
            dispatch_inputs(vec![job("a", 16, "", "")], &mut available, &mut dispatched),
            TEMPLATE,
            &store,
            crate::sizing::global(),
            &provider,
            "gcp",
            &secrets,
            &deployment,
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(created, 0);
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert_eq!(available["nvidia-tesla-t4"], 1);
    }

    #[tokio::test]
    async fn unmeasured_job_recovers_via_smallest_live_vram() {
        let (_dir, store) = store();
        // No live capacity broadcasts at all -> unmeasured job is
        // unschedulable this tick.
        let unsized_job = job("unsized", 0, "", "");
        let buckets = bucket_jobs(
            std::slice::from_ref(&unsized_job),
            &HashMap::new(),
            "gcp",
            crate::sizing::global(),
            &store,
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(buckets.is_empty());

        // A live local agent broadcasting total_vram_gb=16 makes the job
        // recover to the smallest real fleet GPU.
        crate::queue::capacity::publish_capacity(
            &store,
            "local-1",
            "local",
            &BTreeMap::new(),
            None,
            Some(16),
            None,
        )
        .await
        .unwrap();
        let buckets = bucket_jobs(
            &[unsized_job],
            &HashMap::new(),
            "gcp",
            &Sizing::new(),
            &store,
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(
            buckets[0].0,
            ("nvidia-tesla-t4".to_string(), "n1-standard-4".to_string())
        );
    }
}
