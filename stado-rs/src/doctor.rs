//! Deployment preflight probes behind `stado doctor`.
//!
//! NO Python original: the Python CLI has no preflight. Every one of the
//! six blockers in the 2026-07-26 GCP-billing outage surfaced as a crash
//! loop or a silently empty UI instead of a check — an azure backend with
//! no storage account, an all-zero quota, an unreachable release channel,
//! a missing VM managed identity, a startup template that aborted on
//! `set -u`, and a fleet that was simply paused. Each of those is cheap to
//! interrogate directly; none of them was interrogated anywhere.
//!
//! Two properties are load-bearing:
//!
//! - **Fault isolation.** One probe failing must never suppress the rest,
//!   because the useful output is the WHOLE list — the outage looked like
//!   "quota is zero" until the release channel and the VM identity turned
//!   out to be broken too. Every probe captures its own error into its own
//!   [`Check`], exactly as each section of
//!   [`crate::monitor::billing::collect_billing`] captures its own. Probes
//!   additionally run under a shared deadline ([`PROBE_TIMEOUT`]) so a
//!   black-holed endpoint degrades to one FAIL row instead of hanging the
//!   command.
//! - **Same code path as production.** The template probe renders through
//!   [`crate::scheduler::dispatch::agent::bundled_template_for`] with
//!   credentials resolved from Skarbiec and
//!   [`crate::scheduler::dispatch::agent::deployment_substitutions`] — the
//!   dispatcher's own text, secrets and config. A preflight that rendered
//!   its own copy would prove nothing about what dispatch ships.
//!
//! Read-only except for [`check_storage_round_trip`], which is the only
//! answer to "is the queue empty or is the store unreachable" and therefore
//! has to actually write. It writes, reads back and deletes one
//! self-describing object under [`PROBE_PREFIX`], and the delete runs even
//! when the read-back fails — a doctor that litters the queue store on
//! every bad run is worse than no doctor. [`PROBE_PREFIX`] is deliberately
//! outside `queue::copy::CANONICAL_PREFIXES`, for the same reason
//! `queue::copy::SENTINEL_PATH` is: a diagnostic probe is precisely what a
//! backend migration must not carry across.

use std::future::Future;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::config_file;
use crate::coordinator;
use crate::monitor::alerts::AlertChannels;
use crate::providers;
use crate::queue::{control, copy::Endpoint, JobStorage};
use crate::scheduler::dispatch::agent;
use crate::scheduler::quota;
use crate::self_update;
use crate::targets;

/// Prefix the storage round-trip probe writes under. Not a queue-state
/// prefix and deliberately absent from `queue::copy::CANONICAL_PREFIXES`,
/// so a cutover copy never carries a diagnostic object to the new store.
pub const PROBE_PREFIX: &str = "diagnostics/";

/// Provider name of the device-local deployment, which has no cloud API to
/// authenticate against and no agent VMs to dispatch.
/// `crate::coordinator::resolve_providers` skips it for the same reason.
const LOCAL_PROVIDER: &str = crate::capabilities::ProviderId::Local.as_str();

fn provider_enabled(provider: crate::capabilities::ProviderId) -> bool {
    config::wc_providers()
        .iter()
        .any(|name| provider.matches(name))
}

fn storage_adapter(name: &str) -> Option<crate::capabilities::StorageAdapter> {
    crate::capabilities::storage_adapter(name)
}

/// Ceiling on ONE probe. Bounds the command against a black-holed endpoint
/// — the failure mode of an unreachable release channel or a firewalled
/// cloud API, which drop packets rather than refusing them, so the socket
/// never returns. Derived digit-free from `u8::BITS`, the same way
/// `crate::cli::default_mail_results` derives its page size. Probes run
/// concurrently, so this bounds the whole command and not one row of it.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(u8::BITS as u64);

/// Verdict of one check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing to do.
    #[default]
    Pass,
    /// Works, but a documented hazard is live.
    Warn,
    /// Blocking: the deployment cannot do its job in this state.
    Fail,
}

impl Status {
    /// Table rendering.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }

    /// `--json` rendering.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }

    /// The more severe of two verdicts. Lets a check that inspects several
    /// providers reach one verdict without ranking numbers.
    pub const fn worst(self, other: Self) -> Self {
        match (self, other) {
            (Self::Fail, _) | (_, Self::Fail) => Self::Fail,
            (Self::Warn, _) | (_, Self::Warn) => Self::Warn,
            _ => Self::Pass,
        }
    }
}

/// One preflight result. `remedy` is populated on every check, including
/// passing ones, so `--fix-hints` can print the knob that governs a check
/// which currently passes; the default rendering shows it only for WARN
/// and FAIL rows, where it is the actionable part.
#[derive(Debug, Clone)]
pub struct Check {
    /// Stable machine-readable id (`config`, `storage`, ...).
    pub id: &'static str,
    /// Human column heading.
    pub title: &'static str,
    pub status: Status,
    /// What was observed. Carries the EXACT upstream error text on
    /// failure — never a paraphrase, never just "failed".
    pub detail: String,
    /// The env var or command that changes the outcome.
    pub remedy: String,
}

impl Check {
    fn new(
        id: &'static str,
        title: &'static str,
        status: Status,
        detail: String,
        remedy: &str,
    ) -> Self {
        Self {
            id,
            title,
            status,
            detail,
            remedy: remedy.to_string(),
        }
    }

    fn pass(id: &'static str, title: &'static str, detail: String, remedy: &str) -> Self {
        Self::new(id, title, Status::Pass, detail, remedy)
    }

    fn fail(id: &'static str, title: &'static str, detail: String, remedy: &str) -> Self {
        Self::new(id, title, Status::Fail, detail, remedy)
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "status": self.status.key(),
            "detail": self.detail,
            "remedy": self.remedy,
        })
    }
}

/// Per-item outcomes accumulated inside one check that inspects several
/// things (one row per configured provider, say). The check's verdict is
/// the worst of them, so a healthy GCP arm never masks a broken Azure one.
#[derive(Default)]
struct Findings {
    status: Status,
    notes: Vec<String>,
    remedies: Vec<String>,
}

impl Findings {
    fn note(&mut self, status: Status, note: String) {
        self.status = self.status.worst(status);
        self.notes.push(note);
    }

    /// Record the fix for a specific finding, de-duplicated: several
    /// providers failing the same way should not repeat the same remedy.
    fn remedy(&mut self, remedy: impl Into<String>) {
        let remedy = remedy.into();
        if !self.remedies.contains(&remedy) {
            self.remedies.push(remedy);
        }
    }

    /// `base` is the knob that governs this check when nothing went wrong.
    fn into_check(self, id: &'static str, title: &'static str, base: &str) -> Check {
        let remedy = if self.remedies.is_empty() {
            base.to_string()
        } else {
            self.remedies.join(" | ")
        };
        Check {
            id,
            title,
            status: self.status,
            detail: self.notes.join("; "),
            remedy,
        }
    }
}

/// The full preflight outcome, in the order the checks are meant to be
/// read: earliest blocking failure first.
#[derive(Debug, Clone)]
pub struct Report {
    pub generated_at: String,
    pub checks: Vec<Check>,
}

impl Report {
    /// Worst verdict across every check.
    pub fn status(&self) -> Status {
        self.checks
            .iter()
            .fold(Status::Pass, |so_far, check| so_far.worst(check.status))
    }

    /// The first FAIL in preflight order — the one to fix before reading
    /// further, because the later checks are usually downstream of it.
    pub fn first_failure(&self) -> Option<&Check> {
        self.checks
            .iter()
            .find(|check| check.status == Status::Fail)
    }

    pub fn failed(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == Status::Fail)
            .count()
    }

    pub fn warned(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == Status::Warn)
            .count()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "generated_at": self.generated_at,
            "status": self.status().key(),
            "failed": self.failed(),
            "warned": self.warned(),
            "checks": self.checks.iter().map(Check::to_json).collect::<Vec<Value>>(),
        })
    }
}

/// Run a probe under [`PROBE_TIMEOUT`]. An elapsed probe becomes a FAIL
/// row rather than a hung command, so the remaining probes still report.
/// Nothing serving here that is placed somewhere else.
///
/// A gateway is placed on exactly one host. A second copy listening on the same
/// port on another machine does not announce itself: callers that resolve a
/// loopback address reach it, it authenticates against its own stale view, and
/// the refusal reads as a credential fault. Cheap to detect from here, because
/// the only thing to look at is whether this host is holding the port of a
/// service the directory places elsewhere.
async fn check_placement() -> Check {
    let document = match crate::cli::registry::fetch_document().await {
        Ok(document) => document,
        Err(error) => {
            return Check::new(
                PLACEMENT_ID,
                PLACEMENT_TITLE,
                Status::Warn,
                format!("the registry could not be read: {error}"),
                PLACEMENT_REMEDY,
            )
        }
    };
    let here = crate::providers::vast::system_hostname();
    let services = document
        .get("service_directory")
        .and_then(|block| block.get("services"))
        .and_then(serde_json::Value::as_object);
    let Some(services) = services else {
        return Check::pass(
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            "the directory declares no services".to_string(),
            PLACEMENT_REMEDY,
        );
    };
    let mut squatting: Vec<String> = Vec::new();
    for (name, entry) in services {
        let Some(active) = entry.get("active_host").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if active.starts_with(&here) || here.starts_with(active) {
            continue;
        }
        let Some(port) = crate::cli::directory::service_port(entry, active) else {
            continue;
        };
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            squatting.push(format!(
                "{name} is placed on {active}, and port {port} answers here"
            ));
        }
    }
    if squatting.is_empty() {
        Check::pass(
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            "this host holds no port belonging to a service placed elsewhere".to_string(),
            PLACEMENT_REMEDY,
        )
    } else {
        Check::fail(
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            squatting.join("; "),
            PLACEMENT_REMEDY,
        )
    }
}

async fn bounded(
    id: &'static str,
    title: &'static str,
    remedy: &str,
    probe: impl Future<Output = Check>,
) -> Check {
    bounded_within(PROBE_TIMEOUT, id, title, remedy, probe).await
}

/// Run a probe under an explicit deadline. A row whose work grows with the
/// deployment cannot share one flat budget with a row that makes a single
/// call: the gateway sweep reads every mapped item through one listener, and
/// under the flat bound it failed intermittently with "probe did not answer"
/// while the same sweep measured under a second.
async fn bounded_within(
    deadline: Duration,
    id: &'static str,
    title: &'static str,
    remedy: &str,
    probe: impl Future<Output = Check>,
) -> Check {
    match tokio::time::timeout(deadline, probe).await {
        Ok(check) => check,
        Err(_) => Check::fail(
            id,
            title,
            format!("probe did not answer within {deadline:?}"),
            remedy,
        ),
    }
}

/// Per-item allowance for the gateway-auth sweep, multiplied by the number of
/// items the four verifier mappings actually declare.
fn object_auth_deadline() -> Duration {
    // Each mapping resolves independently; one that cannot be read contributes
    // nothing to the sweep, so it contributes nothing to the budget either.
    let mapped = crate::config::object_api_namespaces().map_or(usize::MIN, |items| items.len())
        + crate::config::release_api_publishers().map_or(usize::MIN, |items| items.len())
        + crate::config::machine_api_clients().map_or(usize::MIN, |items| items.len())
        + crate::config::service_api_deployers().map_or(usize::MIN, |items| items.len());
    PROBE_TIMEOUT + PROBE_TIMEOUT * u32::try_from(mapped).unwrap_or_default()
}

/// Allowance for resolving alert channels. Each enabled channel reads its own
/// destination and provider material out of the vault, through the same
/// single-threaded listener the gateway sweep is using at the same moment.
fn alerts_deadline() -> Duration {
    let channels = crate::config::alert_channels().len();
    PROBE_TIMEOUT + PROBE_TIMEOUT * u32::try_from(channels).unwrap_or_default()
}

/// Run the whole preflight. Never returns an error: an unreachable
/// dependency is a FAIL row, not an aborted command.
pub async fn run() -> Report {
    // One facade for every store-backed probe. Its construction failure is
    // itself diagnostic — on the azure backend with an empty account
    // `JobStorage::with_bucket` hard-errors — so each dependent check
    // reports it instead of the whole command dying here.
    let store = JobStorage::new().await;
    let store_error = store
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    let store = store.as_ref().ok();

    // Concurrent, like the two sections of
    // `monitor::billing::live_snapshot`; the fixed assembly order below is
    // what makes the report ordered.
    let (
        config_check,
        storage_check,
        backup_check,
        object_auth_check,
        providers_check,
        quota_check,
        release_check,
        template_check,
        identity_check,
        registry_check,
        control_check,
        alerts_check,
        contract_check,
        placement_check,
    ) = tokio::join!(
        bounded(CONFIG_ID, CONFIG_TITLE, CONFIG_REMEDY, async {
            check_config()
        }),
        bounded(
            STORAGE_ID,
            STORAGE_TITLE,
            STORAGE_REMEDY,
            check_storage_round_trip(store, &store_error)
        ),
        bounded(
            BACKUP_ID,
            BACKUP_TITLE,
            BACKUP_REMEDY,
            check_backup(&store_error)
        ),
        bounded_within(
            object_auth_deadline(),
            OBJECT_AUTH_ID,
            OBJECT_AUTH_TITLE,
            OBJECT_AUTH_REMEDY,
            check_object_auth()
        ),
        bounded(
            PROVIDERS_ID,
            PROVIDERS_TITLE,
            PROVIDERS_REMEDY,
            check_provider_auth()
        ),
        bounded(
            QUOTA_ID,
            QUOTA_TITLE,
            QUOTA_REMEDY,
            check_quota(store, &store_error)
        ),
        bounded(
            RELEASE_ID,
            RELEASE_TITLE,
            RELEASE_REMEDY,
            check_release_channel()
        ),
        bounded(TEMPLATE_ID, TEMPLATE_TITLE, TEMPLATE_REMEDY, async {
            check_agent_template().await
        }),
        bounded(IDENTITY_ID, IDENTITY_TITLE, IDENTITY_REMEDY, async {
            check_vm_identity()
        }),
        bounded(
            REGISTRY_ID,
            REGISTRY_TITLE,
            REGISTRY_REMEDY,
            check_registry()
        ),
        bounded(
            CONTROL_ID,
            CONTROL_TITLE,
            CONTROL_REMEDY,
            check_queue_control(store, &store_error)
        ),
        bounded_within(
            alerts_deadline(),
            ALERTS_ID,
            ALERTS_TITLE,
            ALERTS_REMEDY,
            check_alerts()
        ),
        bounded(
            CONTRACT_ID,
            CONTRACT_TITLE,
            CONTRACT_REMEDY,
            skarbiec_contract_check()
        ),
        bounded(
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            PLACEMENT_REMEDY,
            check_placement(),
        ),
    );

    Report {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        // Preflight order: configuration, then the store everything else
        // reads, then credentials, then capacity, then the two things an
        // agent VM needs in order to exist at all, then fleet identity,
        // then the switches that explain an idle-but-healthy deployment.
        checks: vec![
            config_check,
            storage_check,
            backup_check,
            object_auth_check,
            providers_check,
            quota_check,
            release_check,
            template_check,
            identity_check,
            registry_check,
            control_check,
            alerts_check,
            contract_check,
            placement_check,
        ],
    }
}

// ---------------------------------------------------------------------------
// 1. Config
// ---------------------------------------------------------------------------

const CONFIG_ID: &str = "config";
const CONFIG_TITLE: &str = "Config";
const CONFIG_REMEDY: &str =
    "set provider preference, explicit provider fences and storage locators in the Stado \
     deployment config; `stado config show` prints the resolved set";

/// Resolved backend, providers, storage locator and the config file
/// actually in use.
fn check_config() -> Check {
    let backend = config::wc_storage_backend();
    let mut findings = Findings::default();

    let endpoint = Endpoint::configured_primary();
    let locator = endpoint.describe();
    let config_file = match config_file::config_path() {
        Ok(Some(path)) => path.display().to_string(),
        Ok(None) => "none (env and built-in defaults only)".to_string(),
        Err(err) => format!("unreadable: {err}"),
    };
    findings.note(
        Status::Pass,
        format!(
            "backend={backend} {locator} providers=[{}] disabled=[{}] config_file={config_file}",
            config::wc_providers().join(","),
            config::wc_disabled_providers().join(",")
        ),
    );

    if crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Storage,
        backend,
    )
    .is_none()
    {
        findings.note(
            Status::Fail,
            format!("WC_STORAGE_BACKEND={backend:?} is not a backend this build can construct"),
        );
        let choices =
            crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Storage)
                .collect::<Vec<_>>()
                .join(", ");
        findings.remedy(format!("set storage.backend to one of {choices} in config"));
    }

    if let Some(variant) = crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Storage,
        backend,
    ) {
        for field in variant.config.iter().filter(|field| field.required) {
            if endpoint.locator_value(field.key).is_none_or(str::is_empty) {
                findings.note(
                    Status::Fail,
                    format!(
                        "{} is unresolved while {:?} primary storage is selected",
                        field.env, variant.id
                    ),
                );
                findings.remedy(format!(
                    "set {} in the Stado deployment config ({})",
                    field.path, field.env
                ));
            }
        }
    }

    if config::wc_providers().is_empty() {
        findings.note(
            Status::Fail,
            "WC_PROVIDERS resolved to an empty list".to_string(),
        );
        findings.remedy("configure at least one unfenced provider in preferred order");
    }

    findings.into_check(CONFIG_ID, CONFIG_TITLE, CONFIG_REMEDY)
}

// ---------------------------------------------------------------------------
// 2. Storage auth + round trip
// ---------------------------------------------------------------------------

const STORAGE_ID: &str = "storage";
const STORAGE_TITLE: &str = "Storage round trip";
const STORAGE_REMEDY: &str =
    "the store is selected by WC_STORAGE_BACKEND and its locator vars; provider \
     adapters use their configured workload identity and never fall back to a cloud CLI \
     or a provider-key grant";

/// Object name for one round trip. Unique per run, so two operators
/// running `stado doctor` at once cannot delete each other's probe, and
/// named so whoever finds a leaked one knows what it is without grepping.
fn probe_blob() -> String {
    let host = targets::normalize_hostname(&providers::vast::system_hostname());
    format!(
        "{PROBE_PREFIX}stado-doctor-probe-safe-to-delete-{host}-{}.json",
        uuid::Uuid::new_v4()
    )
}

/// Write, read back and delete one object. The only write `stado doctor`
/// performs, and the only check anywhere that separates "the queue is
/// empty" from "the store is unreachable".
async fn check_storage_round_trip(store: Option<&JobStorage>, store_error: &str) -> Check {
    let Some(store) = store else {
        return Check::fail(
            STORAGE_ID,
            STORAGE_TITLE,
            format!("storage backend could not be constructed: {store_error}"),
            STORAGE_REMEDY,
        );
    };
    let target = format!(
        "backend={} bucket={:?}",
        store.backend_name(),
        store.bucket_name()
    );
    let path = probe_blob();
    let payload = serde_json::to_string_pretty(&json!({
        "written_by": "stado doctor",
        "purpose": "storage auth + round-trip probe",
        "note": "diagnostic only, carries no queue state; safe to delete",
        "written_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        "host": providers::vast::system_hostname(),
    }))
    .expect("probe document serializes");

    // Cleanup is unconditional once a write was attempted: a doctor that
    // leaks an object on every failing run is worse than no doctor. Delete
    // is idempotent, so running it after a failed write costs nothing.
    let written = store.upload_text(&path, &payload).await;
    let read_back = match written {
        Ok(()) => Some(store.download_text(&path).await),
        Err(_) => None,
    };
    let cleaned = store.delete_blob(&path).await;

    let mut findings = Findings::default();
    match (written, read_back) {
        (Err(err), _) => {
            findings.note(
                Status::Fail,
                format!("write of {path} failed ({target}): {err}"),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), Some(Err(err))) => {
            findings.note(
                Status::Fail,
                format!("read back of {path} failed ({target}): {err}"),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), Some(Ok(None))) => {
            findings.note(
                Status::Fail,
                format!(
                    "{path} was written without error but reads back as absent ({target}); the \
                     credentials can write but not read, or the write landed somewhere other \
                     than where the read looks"
                ),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), Some(Ok(Some(text)))) if text != payload => {
            findings.note(
                Status::Fail,
                format!(
                    "{path} read back {} byte(s) instead of the {} written ({target}); the \
                     backend is not returning what it stored",
                    text.len(),
                    payload.len()
                ),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), _) => findings.note(
            Status::Pass,
            format!("wrote, read back and deleted {path} ({target})"),
        ),
    }
    if let Err(err) = cleaned {
        findings.note(
            Status::Warn,
            format!("probe object {path} could NOT be deleted and is still in the store: {err}"),
        );
        findings.remedy(format!("delete the leaked probe object {path} by hand"));
    }
    findings.into_check(STORAGE_ID, STORAGE_TITLE, STORAGE_REMEDY)
}

const BACKUP_ID: &str = "backup";
const BACKUP_TITLE: &str = "Disaster-recovery replica";
const BACKUP_REMEDY: &str =
    "set storage.backup backend/bucket/region and provision the provider adapter's \
     workload identity; backup credentials must not be exposed through agent.skarbiec.items \
     or agent.skarbiec.secret_fields";

async fn check_backup(store_error: &str) -> Check {
    let primary_adapter = storage_adapter(config::wc_storage_backend());
    let backup_adapter = storage_adapter(config::wc_backup_storage_backend());
    let azure_cutover = primary_adapter == Some(crate::capabilities::StorageAdapter::AzureBlob)
        || provider_enabled(crate::capabilities::ProviderId::Azure);
    if !azure_cutover && primary_adapter == Some(crate::capabilities::StorageAdapter::Local) {
        if backup_adapter != Some(crate::capabilities::StorageAdapter::Local)
            || config::wc_backup_local_storage_path().is_empty()
            || config::wc_backup_local_storage_path() == config::wc_local_storage_path()
        {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                "local outage profile needs a distinct storage.backup.local.path; use \
                 ~/.stado/local-backup"
                    .to_string(),
                "configure a distinct local backup path; it is temporary same-disk protection, \
                 not cross-provider disaster recovery",
            );
        }
        let Some(endpoint) = Endpoint::configured_backup() else {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                "local backup endpoint did not resolve".to_string(),
                "set storage.backup.backend=local and storage.backup.local.path",
            );
        };
        let backend = match endpoint.build().await {
            Ok(backend) => backend,
            Err(error) => {
                return Check::fail(
                    BACKUP_ID,
                    BACKUP_TITLE,
                    format!(
                        "local backup cannot be opened at {}: {error}",
                        endpoint.describe()
                    ),
                    "create an owner-writable ~/.stado/local-backup directory",
                )
            }
        };
        if let Err(error) = backend
            .list_blobs_with_meta("diagnostics/backup-access/")
            .await
        {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                format!(
                    "local backup cannot be listed at {}: {error}",
                    endpoint.describe()
                ),
                "create an owner-writable ~/.stado/local-backup directory",
            );
        }
        if !store_error.is_empty() {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                format!("local primary plus backup could not be constructed: {store_error}"),
                "create owner-writable ~/.stado/local-storage and ~/.stado/local-backup directories",
            );
        }
        return Check::pass(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "{} mirrors the local primary with read fallback; this is same-disk temporary \
                 protection only",
                endpoint.describe()
            ),
            "restore required Azure-primary plus S3 cross-provider disaster recovery after the \
             tenant block is removed",
        );
    }
    if !azure_cutover {
        return Check::pass(
            BACKUP_ID,
            BACKUP_TITLE,
            "Azure cutover is not active; no mandatory S3 replica".to_string(),
            BACKUP_REMEDY,
        );
    }
    if backup_adapter != Some(crate::capabilities::StorageAdapter::S3) {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "Azure cutover requires WC_BACKUP_STORAGE_BACKEND=s3, got {:?}; no automatic \
                 writer promotion is permitted",
                config::wc_backup_storage_backend()
            ),
            BACKUP_REMEDY,
        );
    }
    if config::wc_backup_bucket().is_empty() || config::wc_backup_s3_region().is_empty() {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "S3 replica locator unresolved: bucket={:?} region={:?}",
                config::wc_backup_bucket(),
                config::wc_backup_s3_region()
            ),
            BACKUP_REMEDY,
        );
    }
    let Some(endpoint) = Endpoint::configured_backup() else {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            "S3 replica endpoint did not resolve from the configured backup locator".to_string(),
            BACKUP_REMEDY,
        );
    };
    let backend = match endpoint.build().await {
        Ok(backend) => backend,
        Err(error) => {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                format!("S3 replica provider identity could not be resolved: {error}"),
                BACKUP_REMEDY,
            )
        }
    };
    if let Err(error) = backend
        .list_blobs_with_meta("diagnostics/backup-access/")
        .await
    {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "S3 replica provider identity lacks list access at {}: {error}",
                endpoint.describe()
            ),
            BACKUP_REMEDY,
        );
    }
    if !store_error.is_empty() {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "Azure primary plus S3 backup could not be constructed; backup provider \
                 identity, bucket, or primary managed identity is unresolved: {store_error}"
            ),
            BACKUP_REMEDY,
        );
    }
    match coordinator::agent_workload_grant().await {
        Ok(Some(_)) => Check::pass(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "provider adapter can list S3 replica s3://{} in {}; dedicated agent consumer \
                 {:?} exposes exactly its provider-neutral workload items",
                config::wc_backup_bucket(),
                config::wc_backup_s3_region(),
                config::agent_skarbiec_consumer()
            ),
            BACKUP_REMEDY,
        ),
        Ok(None) => Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            "Azure workload grant was not resolved; dispatch is fenced".to_string(),
            BACKUP_REMEDY,
        ),
        Err(error) => Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!("Azure workload grant is absent, unreachable, or overbroad: {error}"),
            BACKUP_REMEDY,
        ),
    }
}

const OBJECT_AUTH_ID: &str = "object-auth";
const OBJECT_AUTH_TITLE: &str = "Object, release, machine, and service gateway auth";
const OBJECT_AUTH_REMEDY: &str =
    "configure object_api.namespaces, release_api.publishers, machine_api.clients, and \
     service_api.deployers; install their distinct owner-only verifier grants and scope each \
     verifier to exactly its mapped items; every mapped item must contain a distinct non-empty \
     token field";

async fn check_object_auth() -> Check {
    let objects = crate::skarbiec::validate_object_verifier().await;
    let releases = crate::skarbiec::validate_release_verifier().await;
    let machines = crate::skarbiec::validate_machine_verifier().await;
    let services = crate::skarbiec::validate_service_verifier().await;
    match (objects, releases, machines, services) {
        (
            Ok(namespace_count),
            Ok(publisher_count),
            Ok(machine_client_count),
            Ok(deployer_count),
        ) => Check::pass(
            OBJECT_AUTH_ID,
            OBJECT_AUTH_TITLE,
            format!(
                "product verifier exposes exactly {namespace_count} namespace items, release \
                 verifier exposes exactly {publisher_count} publisher items, machine verifier \
                 exposes exactly {machine_client_count} client items, and service verifier \
                 exposes exactly {deployer_count} deployer items; tokens are present and \
                 distinct; namespace, prefix, client, target, service, and action policy is valid"
            ),
            OBJECT_AUTH_REMEDY,
        ),
        (object_result, release_result, machine_result, service_result) => {
            let mut failures = Vec::new();
            if let Err(error) = object_result {
                failures.push(format!("product verifier: {error}"));
            }
            if let Err(error) = release_result {
                failures.push(format!("release verifier: {error}"));
            }
            if let Err(error) = machine_result {
                failures.push(format!("machine verifier: {error}"));
            }
            if let Err(error) = service_result {
                failures.push(format!("service verifier: {error}"));
            }
            Check::fail(
                OBJECT_AUTH_ID,
                OBJECT_AUTH_TITLE,
                format!(
                    "authorization fails closed because mapping, verifier grant, or mapped token \
                     validation failed: {}",
                    failures.join("; ")
                ),
                OBJECT_AUTH_REMEDY,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Provider auth
// ---------------------------------------------------------------------------

const PROVIDERS_ID: &str = "providers";
const PROVIDERS_TITLE: &str = "Provider auth";
const PROVIDERS_REMEDY: &str =
    "run each cloud provider only behind its adapter's managed workload identity; static \
     provider keys, cloud CLI sessions, and provider credential items in control-plane or \
     agent grants are unsupported";

/// The cheapest authenticated call each provider offers: list what it is
/// already running. Reports the exact upstream error, because "the
/// credentials are missing" and "the subscription is disabled" read alike
/// from a boolean and need opposite fixes.
async fn check_provider_auth() -> Check {
    let mut findings = Findings::default();
    for name in config::wc_providers() {
        if name == LOCAL_PROVIDER {
            findings.note(
                Status::Pass,
                format!("{name}: device-local, no cloud API to authenticate against"),
            );
            continue;
        }
        let provider = match providers::get_provider(name) {
            Ok(provider) => provider,
            Err(err) => {
                findings.note(
                    Status::Fail,
                    format!("{name}: cannot construct provider: {err}"),
                );
                findings.remedy(PROVIDERS_REMEDY);
                continue;
            }
        };
        match provider.list_running_instances().await {
            Ok(running) => {
                let total: i64 = running.values().sum();
                findings.note(
                    Status::Pass,
                    format!("{name}: authenticated, {total} instance(s) running"),
                );
            }
            Err(err) => {
                findings.note(Status::Fail, format!("{name}: {err}"));
                findings.remedy(PROVIDERS_REMEDY);
            }
        }
    }
    if findings.notes.is_empty() {
        findings.note(
            Status::Fail,
            "WC_PROVIDERS lists no provider to check".to_string(),
        );
        let choices =
            crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Compute)
                .collect::<Vec<_>>()
                .join(", ");
        findings.remedy(format!(
            "set WC_PROVIDERS to one or more comma-separated providers: {choices}"
        ));
    }
    findings.into_check(PROVIDERS_ID, PROVIDERS_TITLE, PROVIDERS_REMEDY)
}

// ---------------------------------------------------------------------------
// 4. Quota
// ---------------------------------------------------------------------------

const QUOTA_ID: &str = "quota";
const QUOTA_TITLE: &str = "Quota";
const QUOTA_REMEDY: &str =
    "`stado quota show` prints the live picture; raise a ceiling with `stado quota request \
     --accel <ACCEL> --new-limit <N>`, and check the reservation overlay at config/quotas.json \
     in the queue store";

/// Live per-accelerator quota through [`quota::load_quotas`] — the same
/// call the dispatcher's admission control makes. Nothing schedulable
/// anywhere is a hard FAIL: no bucket can ever be dispatched, which is
/// exactly what an all-zero Azure subscription looks like from outside.
async fn check_quota(store: Option<&JobStorage>, store_error: &str) -> Check {
    let Some(store) = store else {
        return Check::fail(
            QUOTA_ID,
            QUOTA_TITLE,
            format!(
                "quota needs the reservation overlay from the queue store, which could not be \
                 constructed: {store_error}"
            ),
            STORAGE_REMEDY,
        );
    };
    let mut findings = Findings::default();
    let mut any_capacity = false;
    for name in config::wc_providers() {
        if name == LOCAL_PROVIDER {
            // A device-local deployment schedules on the box's own GPU, so
            // it is real capacity even with no cloud quota anywhere.
            findings.note(
                Status::Pass,
                format!("{name}: device-local, admission is by live VRAM not cloud quota"),
            );
            any_capacity = true;
            continue;
        }
        match quota::load_quotas(store, name).await {
            Err(err) => {
                findings.note(Status::Fail, format!("{name}: {err}"));
                findings.remedy(QUOTA_REMEDY);
            }
            Ok(document) => {
                let rows = document.get(name).and_then(Value::as_object);
                let Some(rows) = rows.filter(|rows| !rows.is_empty()) else {
                    findings.note(
                        Status::Fail,
                        format!("{name}: the quota API reported no accelerator at all"),
                    );
                    findings.remedy(QUOTA_REMEDY);
                    continue;
                };
                let mut totals: Vec<String> = Vec::new();
                let mut provider_capacity = false;
                for (accel, row) in rows {
                    let total = row.get("total").and_then(Value::as_i64).unwrap_or_default();
                    let reserved = row
                        .get("reserved")
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    provider_capacity |= (total - reserved).is_positive();
                    totals.push(format!("{accel}={total}(-{reserved} reserved)"));
                }
                any_capacity |= provider_capacity;
                let status = if provider_capacity {
                    Status::Pass
                } else {
                    Status::Warn
                };
                findings.note(status, format!("{name}: {}", totals.join(" ")));
            }
        }
    }
    if !any_capacity {
        findings.note(
            Status::Fail,
            "no accelerator has a schedulable slot on any configured provider; every dispatch \
             attempt fails admission and the fleet stays at zero VMs"
                .to_string(),
        );
        findings.remedy(QUOTA_REMEDY);
    }
    findings.into_check(QUOTA_ID, QUOTA_TITLE, QUOTA_REMEDY)
}

// ---------------------------------------------------------------------------
// 5. Release channel
// ---------------------------------------------------------------------------

const RELEASE_ID: &str = "release";
const RELEASE_TITLE: &str = "Release channel";
const RELEASE_REMEDY: &str = "set STADO_RELEASE_API_URL plus exact STADO_RELEASE_VERSION and \
     STADO_RELEASE_PLATFORM (config keys release.api_url, release.version, and \
     release.platform); publish both the binary and SHA256SUMS at the canonical \
     stado://releases/stado/<version>/<platform>/ prefix";

/// GET the exact release checksum manifest through the same public Stado route
/// used by agent startup. A missing coordinate, route failure, malformed
/// manifest, or absent binary checksum is a hard failure before dispatch.
async fn check_release_channel() -> Check {
    let api = config::stado_release_api_url();
    let version = config::stado_release_version();
    let platform = config::stado_release_platform();
    let local_only = config::wc_providers()
        .iter()
        .all(|provider| provider == LOCAL_PROVIDER);
    if local_only && api.is_empty() && version.is_empty() && platform.is_empty() {
        return Check::pass(
            RELEASE_ID,
            RELEASE_TITLE,
            "local-only outage profile uses the installed Rust binary; no cloud VM release is active"
                .to_string(),
            RELEASE_REMEDY,
        );
    }

    let mut findings = Findings::default();
    if !api.starts_with("https://") || version.is_empty() || platform.is_empty() {
        findings.note(
            Status::Fail,
            "the public release API, exact version, and exact platform must all be configured"
                .to_string(),
        );
        findings.remedy(RELEASE_REMEDY);
        return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
    }

    let uri = format!("stado://releases/stado/{version}/{platform}/SHA256SUMS");
    let endpoint = format!("{api}/api/release/object");
    let response = match reqwest::Client::new()
        .get(&endpoint)
        .query(&[("uri", uri.as_str())])
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("exact release manifest {uri} is unreachable: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
            return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
        }
    };
    if !response.status().is_success() {
        findings.note(
            Status::Fail,
            format!(
                "exact release manifest {uri} returned HTTP {}",
                response.status()
            ),
        );
        findings.remedy(RELEASE_REMEDY);
        return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("cannot read exact release manifest {uri}: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
            return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
        }
    };
    match self_update::parse_sha256sums(&body) {
        Ok(sums) if sums.contains_key("stado") => findings.note(
            Status::Pass,
            format!("{uri} is immutable, reachable, and supplies the stado checksum"),
        ),
        Ok(_) => {
            findings.note(
                Status::Fail,
                format!("{uri} has no checksum for stado; agent installation fails closed"),
            );
            findings.remedy(RELEASE_REMEDY);
        }
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("{uri} is not a valid checksum manifest: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
        }
    }
    findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY)
}

// ---------------------------------------------------------------------------
// 6. Agent template render
// ---------------------------------------------------------------------------

const TEMPLATE_ID: &str = "template";
const TEMPLATE_TITLE: &str = "Agent template";
const TEMPLATE_REMEDY: &str = "publish one immutable Python/model runtime bundle and configure \
     STADO_AGENT_RUNTIME_BUNDLE_URI + STADO_AGENT_RUNTIME_BUNDLE_SHA256; \
     configure deployment storage/backup and dedicated agent.skarbiec settings. \
     Every template must export the full scheduler-owned placeholder contract";

/// A representative accelerator for `provider`: the smallest VRAM tier of
/// its [`GPU_SIZING`] ladder. Deterministic (`BTreeMap` order) and real,
/// and its absence doubles as "this provider dispatches no agent VMs".
fn representative_accel(provider: &str) -> Option<&'static str> {
    GPU_SIZING
        .get(provider)?
        .values()
        .next()
        .map(|(_machine_type, accel)| *accel)
}

/// Render each configured provider's startup script exactly as dispatch
/// would and assert no `${PLACEHOLDER}` survives.
///
/// This is the check that would have caught the Azure cutover: the
/// template exports `${WC_STORAGE_BACKEND}` and friends under `set -u`,
/// no producer supplied them, and every dispatched VM aborted before the
/// agent started — billing for instances that ran nothing.
async fn check_agent_template() -> Check {
    if config::wc_providers()
        .iter()
        .all(|provider| representative_accel(provider).is_none())
    {
        return Check::pass(
            TEMPLATE_ID,
            TEMPLATE_TITLE,
            "active providers dispatch no cloud agent VMs; no startup template credentials are \
             required"
                .to_string(),
            TEMPLATE_REMEDY,
        );
    }
    let secrets = match coordinator::secrets_from_skarbiec().await {
        Ok(secrets) => secrets,
        Err(err) => {
            return Check::fail(
                TEMPLATE_ID,
                TEMPLATE_TITLE,
                format!("cannot resolve template credentials from Skarbiec: {err}"),
                TEMPLATE_REMEDY,
            )
        }
    };
    let mut findings = Findings::default();
    for name in config::wc_providers() {
        let Some(accel) = representative_accel(name) else {
            findings.note(
                Status::Pass,
                format!("{name}: dispatches no agent VMs, no startup template to render"),
            );
            continue;
        };
        let Some(template) = agent::bundled_template_for(name) else {
            findings.note(
                Status::Fail,
                format!("{name}: no execution template registered in the capability catalog"),
            );
            findings.remedy(TEMPLATE_REMEDY);
            continue;
        };
        let deployment = agent::deployment_substitutions(name);
        match agent::render_agent_startup_script(name, template, accel, &secrets, &deployment) {
            Ok(script) => findings.note(
                Status::Pass,
                format!(
                    "{name}: rendered {} byte(s) with complete storage, scoped-grant, and \
                     immutable-runtime exports for {accel}",
                    script.len()
                ),
            ),
            Err(err) => {
                findings.note(Status::Fail, format!("{name} ({accel}): {err}"));
                findings.remedy(TEMPLATE_REMEDY);
            }
        }
    }
    findings.into_check(TEMPLATE_ID, TEMPLATE_TITLE, TEMPLATE_REMEDY)
}

// ---------------------------------------------------------------------------
// 7. VM identity
// ---------------------------------------------------------------------------

const IDENTITY_ID: &str = "vm-identity";
const IDENTITY_TITLE: &str = "Azure VM identity";
const IDENTITY_REMEDY: &str =
    "export AZURE_VM_IDENTITY_ID=/subscriptions/<sub>/resourceGroups/<rg>/providers/\
     Microsoft.ManagedIdentity/userAssignedIdentities/<name> (config key \
     azure.vm_identity_id), and grant that identity read/write on the queue container";

/// Without a user-assigned identity on the VM, the on-VM half of the
/// [`crate::azure_token`] chain resolves nothing: an agent VM carries no
/// service-principal env vars and no `az` CLI, so IMDS is the only source
/// left and IMDS answers only for a VM that has an identity attached. The
/// agent can then neither read the queue nor self-delete, so it bills
/// until an operator happens to notice.
fn check_vm_identity() -> Check {
    if !provider_enabled(crate::capabilities::ProviderId::Azure) {
        return Check::pass(
            IDENTITY_ID,
            IDENTITY_TITLE,
            "azure is not in WC_PROVIDERS; no agent VM needs a managed identity".to_string(),
            IDENTITY_REMEDY,
        );
    }
    let identity = config::azure_vm_identity_id();
    if identity.is_empty() {
        return Check::fail(
            IDENTITY_ID,
            IDENTITY_TITLE,
            "AZURE_VM_IDENTITY_ID is empty while azure is in WC_PROVIDERS; VM create emits no \
             identity block, so the agent's IMDS token chain resolves nothing and it can \
             neither claim jobs nor self-delete"
                .to_string(),
            IDENTITY_REMEDY,
        );
    }
    Check::pass(
        IDENTITY_ID,
        IDENTITY_TITLE,
        format!("agent VMs get user-assigned identity {identity}"),
        IDENTITY_REMEDY,
    )
}

// ---------------------------------------------------------------------------
// 8. Registry
// ---------------------------------------------------------------------------

const REGISTRY_ID: &str = "registry";
const REGISTRY_TITLE: &str = "Registry";
const REGISTRY_REMEDY: &str =
    "`stado registry pull` shows what the canonical registry says and `stado registry self` \
     resolves this host; add or rename the entry, then `stado registry validate` and \
     `stado registry push`";

/// The canonical registry must be reachable, must parse, and must know
/// either this host or an active coordinator. An unreachable registry is
/// an error rather than an empty one — [`targets::fetch_registry_remote`]
/// draws that distinction because "the store is down" and "you were
/// removed from the fleet" demand opposite responses.
async fn check_registry() -> Check {
    let registry = match targets::fetch_registry_remote().await {
        Ok(registry) => registry,
        Err(err) => {
            return Check::fail(
                REGISTRY_ID,
                REGISTRY_TITLE,
                err.to_string(),
                REGISTRY_REMEDY,
            )
        }
    };
    let hostname = providers::vast::system_hostname();
    let coordinators: Vec<&str> = registry
        .coordinators
        .iter()
        .filter(|coordinator| coordinator.active)
        .map(|coordinator| coordinator.name.as_str())
        .collect();
    let shape = format!(
        "{} target(s), {} coordinator(s)",
        registry.targets.len(),
        registry.coordinators.len()
    );

    match registry.lookup_self(&hostname) {
        Err(err) => Check::fail(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!("parsed ({shape}) but this host's identity is not resolvable: {err}"),
            REGISTRY_REMEDY,
        ),
        Ok(Some(target)) => Check::pass(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!(
                "reachable, parsed ({shape}); {hostname} is target {:?} of kind {}",
                target.name, target.kind
            ),
            REGISTRY_REMEDY,
        ),
        Ok(None) if !coordinators.is_empty() => Check::pass(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!(
                "reachable, parsed ({shape}); {hostname} is not a target, active \
                 coordinator(s): {}",
                coordinators.join(",")
            ),
            REGISTRY_REMEDY,
        ),
        Ok(None) => Check::fail(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!(
                "reachable and parsed ({shape}) but names neither {hostname} nor any active \
                 coordinator; a daemon started here fails its identity lookup and exits on \
                 every respawn"
            ),
            REGISTRY_REMEDY,
        ),
    }
}

// ---------------------------------------------------------------------------
// 9. Queue control
// ---------------------------------------------------------------------------

const PLACEMENT_ID: &str = "placement";
const PLACEMENT_TITLE: &str = "Service placement";
const PLACEMENT_REMEDY: &str =
    "`stado service stop NAME --host HOST` ends an instance nothing placed here";

const CONTROL_ID: &str = "queue-control";
const CONTROL_TITLE: &str = "Queue control";
const CONTROL_REMEDY: &str = "`stado queue pause` / `stado queue resume` own this state";

/// Whether dispatch is paused. A paused queue perfectly explains an idle
/// fleet in front of a full queue, and is invisible everywhere else.
async fn check_queue_control(store: Option<&JobStorage>, store_error: &str) -> Check {
    let Some(store) = store else {
        return Check::fail(
            CONTROL_ID,
            CONTROL_TITLE,
            format!(
                "the pause flag lives at {} in the queue store, which could not be \
                 constructed: {store_error}",
                control::CONTROL_BLOB
            ),
            STORAGE_REMEDY,
        );
    };
    match control::read(store).await {
        Err(err) => Check::fail(
            CONTROL_ID,
            CONTROL_TITLE,
            format!("could not read {}: {err}", control::CONTROL_BLOB),
            STORAGE_REMEDY,
        ),
        Ok(state) if state.paused => Check::new(
            CONTROL_ID,
            CONTROL_TITLE,
            Status::Warn,
            // The same one-liner the scheduler and the agent print when
            // they refuse work, so all three say the pause the same way.
            format!("dispatch is PAUSED — {}", state.pause_summary()),
            "`stado queue resume` restarts dispatch",
        ),
        Ok(_) => Check::pass(
            CONTROL_ID,
            CONTROL_TITLE,
            "dispatch is running (not paused)".to_string(),
            CONTROL_REMEDY,
        ),
    }
}

// ---------------------------------------------------------------------------
// 10. Alerts
// ---------------------------------------------------------------------------

const ALERTS_ID: &str = "alerts";
const ALERTS_TITLE: &str = "Alerts";
const ALERTS_REMEDY: &str =
    "configure at least one non-GCP channel: enable it in alerts.channels and give it its \
     material - slack_webhook, telegram_bot_token + telegram_chat_id, or sendgrid_api_key in \
     the stado-alerts Skarbiec item, or resend with email_to there and the RESEND_API_KEY \
     item; clear WC_ALERTS_TOPIC on a deployment that has left GCP";

/// At least one alert channel that survives the cloud going away.
///
/// The outage's compounding failure: the only configured channel was GCP
/// Pub/Sub, on the very account whose billing had been disabled, so every
/// alert about the outage failed to send because of the outage.
async fn check_alerts() -> Check {
    // An empty topic short-circuits the Pub/Sub arm of `from_env`, so this
    // resolves the three non-GCP channels through the production logic
    // without paying for (or logging) a GCP token probe.
    let channels = AlertChannels::from_env("").await;
    let mut configured: Vec<&str> = Vec::new();
    if channels.slack_webhook.is_some() {
        configured.push("slack");
    }
    if channels.telegram.is_some() {
        configured.push("telegram");
    }
    if channels.sendgrid.is_some() {
        configured.push("sendgrid");
    }
    if channels.resend.is_some() {
        configured.push("resend");
    }
    if channels.most.is_some() {
        configured.push("most");
    }

    // A resolved channel is not a working one. The provider is the only
    // authority on whether this key is still valid and whether it may send as
    // this sender, and asking costs one read: the deployment sat green for
    // weeks holding a key Resend had already revoked.
    let resend_problem = match &channels.resend {
        Some(resend) => {
            let client = reqwest::Client::new();
            match crate::monitor::alerts::resend_verified_domains(&client, resend).await {
                Ok(domains) => {
                    let sender_domain = resend.from.rsplit('@').next().unwrap_or_default();
                    if domains.iter().any(|domain| domain == sender_domain) {
                        None
                    } else {
                        configured.retain(|channel| *channel != "resend");
                        Some(format!(
                            "resend sender {} is not on a verified domain; verified: [{}]",
                            resend.from,
                            domains.join(",")
                        ))
                    }
                }
                Err(error) => {
                    configured.retain(|channel| *channel != "resend");
                    Some(format!("resend key was refused by the provider: {error}"))
                }
            }
        }
        None => None,
    };

    let topic = config::alerts_topic();
    // "On GCP" means there is still a GCP surface a Pub/Sub publish could
    // plausibly authenticate against: the GCS queue store or the GCP
    // dispatch provider. The billing outage removed both at once.
    let on_gcp = storage_adapter(config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::Gcs)
        || provider_enabled(crate::capabilities::ProviderId::Gcp);
    let mut findings = Findings::default();

    if configured.is_empty() {
        let detail = if topic.is_empty() {
            "no alert channel is configured at all; nothing anywhere will page an operator"
                .to_string()
        } else if on_gcp {
            format!(
                "the only channel is GCP Pub/Sub ({topic}); an outage of that account takes the \
                 alerts down with it, which is exactly how the last one went unnoticed"
            )
        } else {
            format!(
                "the only channel is GCP Pub/Sub ({topic}) but this deployment has no GCP \
                 surface left (backend={}, providers=[{}]); every alert is delivered nowhere",
                config::wc_storage_backend(),
                config::wc_providers().join(",")
            )
        };
        let status = if topic.is_empty() || !on_gcp {
            Status::Fail
        } else {
            Status::Warn
        };
        findings.note(status, detail);
        findings.remedy(ALERTS_REMEDY);
    } else {
        findings.note(
            Status::Pass,
            format!("non-GCP channel(s) configured: {}", configured.join(",")),
        );
    }

    if let Some(problem) = resend_problem {
        findings.note(Status::Fail, problem);
        findings.remedy(
            "point alerts.resend_item at an item holding a key the provider accepts, and \
             alerts.email_from at a verified sending domain; `stado alerts channels` shows \
             what resolved and `stado alerts send` proves delivery",
        );
    }

    if !topic.is_empty() && !on_gcp {
        findings.note(
            Status::Warn,
            format!(
                "WC_ALERTS_TOPIC is set to {topic} on a deployment with no GCP surface; every \
                 send_alert pays a failing gcp_auth probe before the working channels fire"
            ),
        );
        findings.remedy("unset WC_ALERTS_TOPIC (config key alerts.topic) on this deployment");
    }

    findings.into_check(ALERTS_ID, ALERTS_TITLE, ALERTS_REMEDY)
}

const CONTRACT_ID: &str = "skarbiec-contract";
const CONTRACT_TITLE: &str = "Skarbiec read contract";
const CONTRACT_REMEDY: &str =
    "move whole-item reads to Client::read_field, or pin the broker to the build these callers expect";

/// Which read contract is the configured broker enforcing?
///
/// Skarbiec 9aa7dd4 made `field` mandatory on `/v1/items/read`. Callers that
/// asked for an item and picked fields out of it got `400 {"error":"field
/// required"}` with no hint that the contract had moved underneath them. On
/// 2026-08-04 that took out this machine's host-health beacon for twenty-one
/// hours, during which `stado service list` went on reporting a stale
/// `active` for services that were not running.
///
/// Every credential caller in this build now names its field - alerts, the
/// gateway verifiers, azure, cloudflare, billing, the dashboard's operator
/// auth and the fleet key store - so a broker demanding one is the contract
/// this build speaks, and the row says which contract is in force rather than
/// warning about a mismatch that no longer exists.
///
/// The probe is unauthenticated on purpose: the handler validates `id` and
/// `field` before it looks at any identity, so a request carrying neither a
/// consumer nor a bearer still reveals which contract is in force, and reveals
/// nothing else.
async fn skarbiec_contract_check() -> Check {
    let url = match crate::credential_store::skarbiec_url() {
        Some(url) => url,
        // A file-backed credential store has no broker and no contract to
        // disagree with, which is a different thing from a broker that is
        // fine.
        None => {
            return Check::pass(
                CONTRACT_ID,
                CONTRACT_TITLE,
                "credential store is not a Skarbiec broker; no read contract applies".to_string(),
                CONTRACT_REMEDY,
            )
        }
    };
    let endpoint = format!("{}/v1/items/read", url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return Check::new(
                CONTRACT_ID,
                CONTRACT_TITLE,
                Status::Warn,
                format!("could not build an HTTP client to probe {endpoint}: {err}"),
                CONTRACT_REMEDY,
            )
        }
    };
    let response = client
        .post(&endpoint)
        .json(&json!({"id": "stado-doctor-contract-probe"}))
        .send()
        .await;
    match response {
        Err(err) => Check::new(
            CONTRACT_ID,
            CONTRACT_TITLE,
            Status::Warn,
            format!("{endpoint} is unreachable, so the read contract is unknown: {err}"),
            CONTRACT_REMEDY,
        ),
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if body.contains("field required") {
                Check::pass(
                    CONTRACT_ID,
                    CONTRACT_TITLE,
                    format!(
                        "{endpoint} requires a named field (HTTP {status}), which is what this \
                         build sends: every credential read names its field"
                    ),
                    CONTRACT_REMEDY,
                )
            } else {
                Check::new(
                    CONTRACT_ID,
                    CONTRACT_TITLE,
                    Status::Warn,
                    format!(
                        "{endpoint} accepts a read without a named field (HTTP {status}); this \
                         broker predates the field requirement, so an item read here returns \
                         whatever the grant allows rather than the one field asked for"
                    ),
                    CONTRACT_REMEDY,
                )
            }
        }
    }
}
