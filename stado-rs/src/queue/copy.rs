//! Backend-to-backend copy of the whole queue store.
//!
//! NO Python original: this module is new in the Rust runtime, so there is
//! no parity note to make. In-store migrations handle schema changes, while
//! this command is the sole supported cross-backend transfer path. It builds
//! both explicit endpoints inside their provider adapters and never relies on
//! an operator cloud CLI or ambient ADC copier.
//!
//! Both ends are built from explicit locators through [`Endpoint::build`],
//! never through [`crate::queue::JobStorage`]: the facade resolves its
//! backend from the ambient `WC_STORAGE_BACKEND`, so source and destination
//! could never differ inside one process. The credentials path is
//! unchanged — [`Endpoint::build`] calls the same backend constructors the
//! facade calls.
//!
//! What this guarantees:
//!
//! - [`copy`] itself never deletes at either end. The coordinator-only
//!   [`replicate_configured_backup`] reconciliation prunes stale objects from
//!   the designated backup after a fully clean copy; it never deletes source.
//! - **Metadata travels with the body.** `JobStorage::write_job` stamps
//!   `gpu_mem_gb` / `priority` / `gpu_type` on every queue blob in a
//!   separate `set_metadata` round trip, and
//!   `queue::listing::list_fitting` prefilters scheduling on those keys
//!   before downloading anything. A body-only copy would silently degrade
//!   every scheduler tick into downloading the whole queue, so each object
//!   gets its source metadata re-applied at the destination — and the
//!   result is verified, because
//!   `<AzureBlobBackend as BlobBackend>::set_metadata` logs and SWALLOWS
//!   every write failure (Python parity) and therefore proves nothing by
//!   returning `Ok`.
//! - **Resumable and idempotent.** A [`SENTINEL_PATH`] marker in the
//!   DESTINATION store records the last cleanly finished prefix plus the
//!   running counts (same shape as the `queue_priority/.migration.json`
//!   sentinel in `queue/migrations.rs`), and every object that already
//!   exists at the destination with the same size is skipped.
//!
//! Cost note: [`BlobBackend`] exposes no object size — [`BlobInfo`] carries
//! only name, timestamp and metadata — so the "same size" test reads the
//! body at both ends. Re-runs are therefore cheap in WRITES, not in READS.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::StreamExt;

use super::{construct_backend, BackendLocator, BlobBackend, BlobInfo, StorageError};
use crate::capabilities::{RuntimeFacet, StorageAdapter};

/// Every prefix that makes up the queue store, in copy order.
///
/// Hardcoded rather than discovered, because a bare root listing of a live
/// bucket is both enormous and racy, and because two of these entries are
/// invisible to the obvious enumeration:
///
/// - `cancelled/` is NOT one of the prefixes `JobStorage::list_all_jobs`
///   walks (it covers queue/running/completed/uploaded/failed only), so a
///   copy driven by the job listing would silently drop every cancelled
///   job. That is precisely why it is spelled out here.
/// - `queue_priority/` carries the `.migration.json` sentinel of
///   `queue::migrations` alongside the priority markers. It is copied with
///   the rest of the prefix so the destination inherits the completed
///   backfill instead of re-running it.
/// - `ecosystem/` is the provider-neutral product object data plane. Queue
///   migration and disaster-recovery backup must carry it with scheduler
///   state or migrated jobs would point at objects left in the failed store.
///
/// `registry.json` is a root object, not a directory; it is listed as a
/// prefix because every [`BlobBackend`] listing is a plain string-prefix
/// match, so the full object name selects exactly that one object.
pub const CANONICAL_PREFIXES: &[&str] = &[
    "queue/",
    "running/",
    "completed/",
    "uploaded/",
    "failed/",
    "cancelled/",
    "queue_priority/",
    "scripts/",
    "status/",
    "capacity/",
    "provider-leases/",
    "runs/",
    "fixed/",
    "failed_again/",
    "schedules/",
    "cancellations/",
    "machine_requests/",
    "machine_inputs/",
    "config/",
    "state/",
    "operations/",
    "failure_fixes/",
    "coverage/",
    "host_health/",
    "billing_health/",
    "hf_rate/",
    "artifacts/",
    "ecosystem/",
    "registry.json",
];

/// Resume sentinel, written to the DESTINATION store. Mirrors
/// `queue::migrations::SENTINEL_PATH`; deliberately outside
/// [`CANONICAL_PREFIXES`] so a later copy never treats it as queue state.
pub const SENTINEL_PATH: &str = "storage_copy/.copy.json";

/// Default copy fan-out. Reuses the crate's existing bulk-download budget
/// (`queue::migrations::BULK_WORKERS`) rather than inventing a second
/// concurrency style.
pub const DEFAULT_CONCURRENCY: usize = super::migrations::BULK_WORKERS;

/// One end of the copy: which backend to build and the locators it needs.
/// Unused fields for the selected `kind` are ignored.
#[derive(Clone, Debug, Default)]
pub struct Endpoint {
    /// Backend selector: "gcs", "azure", "s3" or "local".
    pub kind: String,
    /// GCS or S3 bucket.
    pub bucket: String,
    /// Azure storage account.
    pub account: String,
    /// Azure container.
    pub container: String,
    /// S3 region; empty defers to the AWS default chain.
    pub region: String,
    /// Local backend root directory.
    pub path: String,
}

impl Endpoint {
    pub fn adapter(&self) -> Option<StorageAdapter> {
        crate::capabilities::storage_adapter(&self.kind)
    }

    /// Build the backend directly from the locators — the same constructors
    /// `JobStorage::with_bucket` uses, without its `WC_STORAGE_BACKEND`
    /// lookup, so a source and a destination of different kinds coexist in
    /// one process.
    pub async fn build(&self) -> Result<Arc<dyn BlobBackend>, StorageError> {
        let variant = crate::capabilities::constructible_variant(RuntimeFacet::Storage, &self.kind)
            .ok_or_else(|| {
                let choices = crate::capabilities::configurable_ids(RuntimeFacet::Storage)
                    .collect::<Vec<_>>()
                    .join("\", \"");
                StorageError::Other(format!(
                    "unknown storage backend {:?} (use \"{choices}\")",
                    self.kind
                ))
            })?;
        let Some(adapter) = self.adapter() else {
            return Err(StorageError::Other(format!(
                "storage variant {:?} has no storage adapter",
                variant.id
            )));
        };
        if adapter == StorageAdapter::Gcs && self.bucket.is_empty() {
            return Err(StorageError::Other(
                "the gcs endpoint needs a bucket (--from-bucket / --to-bucket)".into(),
            ));
        }
        construct_backend(
            adapter,
            BackendLocator {
                bucket: &self.bucket,
                account: &self.account,
                container: &self.container,
                region: &self.region,
                path: &self.path,
            },
        )
        .await
    }

    /// Operator-readable locator for the report header.
    pub fn describe(&self) -> String {
        match self.adapter() {
            Some(StorageAdapter::Gcs) => format!("gcs://{}", self.bucket),
            Some(StorageAdapter::AzureBlob) => {
                format!("azure://{}/{}", self.account, self.container)
            }
            Some(StorageAdapter::S3) => format!("s3://{}", self.bucket),
            Some(StorageAdapter::StadoObject) => {
                format!("stado://{}", crate::config::wc_stado_storage_namespace())
            }
            Some(StorageAdapter::Local) => format!("local://{}", self.path),
            None => self.kind.clone(),
        }
    }

    /// The value behind one configuration key of this endpoint, for callers that
    /// check a backend is fully configured before using it.
    ///
    /// The first five keys are per-endpoint, because a copy has a source and a
    /// destination that differ in exactly those. The Stado object store has none of
    /// them: it is addressed by a URL, a token file and a namespace that are global
    /// to the process, which is why `describe` above already reads them from config
    /// rather than from `self`. Answering `None` for them made every required field
    /// of that backend look unset, so `stado doctor` reported the primary store as
    /// misconfigured on the same run in which it wrote, read back and deleted a probe
    /// object through it. A check that contradicts the check below it teaches
    /// operators to ignore both.
    pub fn locator_value(&self, key: &str) -> Option<&str> {
        match key {
            "bucket" => Some(&self.bucket),
            "account" => Some(&self.account),
            "container" => Some(&self.container),
            "region" => Some(&self.region),
            "path" => Some(&self.path),
            "url" => Some(crate::config::wc_stado_storage_url()),
            "token-file" => Some(crate::config::wc_stado_storage_token_file()),
            "namespace" => Some(crate::config::wc_stado_storage_namespace()),
            "ca-file" => Some(crate::config::wc_stado_storage_ca_file()),
            _ => None,
        }
    }

    /// Resolve the active queue store into the explicit endpoint shape used
    /// by cross-backend operations.
    pub fn configured_primary() -> Self {
        Self::from_locators(
            crate::config::wc_storage_backend(),
            crate::config::bucket(),
            crate::config::wc_azure_storage_account(),
            crate::config::wc_azure_container(),
            crate::config::wc_s3_region(),
            crate::config::wc_local_storage_path(),
        )
    }

    /// Resolve the independently configured disaster-recovery store.
    ///
    /// Empty backend means there is no Stado-managed backup. The returned
    /// endpoint is never selected by `JobStorage`; callers must explicitly
    /// copy to it or inspect it.
    pub fn configured_backup() -> Option<Self> {
        let kind = crate::config::wc_backup_storage_backend();
        if kind.is_empty() {
            return None;
        }
        Some(Self::from_locators(
            kind,
            crate::config::wc_backup_bucket(),
            crate::config::wc_backup_azure_storage_account(),
            crate::config::wc_backup_azure_container(),
            crate::config::wc_backup_s3_region(),
            crate::config::wc_backup_local_storage_path(),
        ))
    }

    fn from_locators(
        kind: &str,
        bucket: &str,
        account: &str,
        container: &str,
        region: &str,
        path: &str,
    ) -> Self {
        Self {
            kind: kind.to_string(),
            bucket: bucket.to_string(),
            account: account.to_string(),
            container: container.to_string(),
            region: region.to_string(),
            path: path.to_string(),
        }
    }
}

/// Knobs for one copy run.
#[derive(Clone, Debug)]
pub struct CopyOptions {
    /// Prefixes to copy; empty selects all of [`CANONICAL_PREFIXES`].
    pub prefixes: Vec<String>,
    /// Objects copied in parallel.
    pub concurrency: usize,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            prefixes: Vec::new(),
            concurrency: DEFAULT_CONCURRENCY,
        }
    }
}

/// What happened to one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Body written to the destination, metadata re-applied and verified.
    Copied,
    /// Body was already identical; only the metadata had to be re-applied.
    /// This is the repair path for a swallowed Azure metadata write.
    MetadataRepaired,
    /// Already at the destination with the same size and metadata.
    Skipped,
    /// Listed at the source but gone by the time it was read — a live queue
    /// moving a job between prefixes mid-copy. Not an error, but reported:
    /// it is the observable symptom of copying an undrained fleet.
    Vanished,
    /// Copy or verification failed; the object is named in the report.
    Failed(String),
}

/// Per-object result.
#[derive(Clone, Debug)]
pub struct ObjectReport {
    pub name: String,
    /// Body bytes credited to this object. Zero unless it came out of the
    /// run verified, so a failed verification never inflates the total.
    pub bytes: u64,
    pub outcome: Outcome,
}

/// Per-prefix result.
#[derive(Clone, Debug)]
pub struct PrefixReport {
    pub prefix: String,
    /// Set when the prefix could not be listed at all; its objects are then
    /// unknown and nothing under it was copied.
    pub listing_error: Option<String>,
    pub objects: Vec<ObjectReport>,
}

impl PrefixReport {
    /// Objects whose body was written.
    pub fn copied(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Copied))
    }

    /// Objects whose body was already correct but whose metadata was
    /// (re-)applied.
    pub fn repaired(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::MetadataRepaired))
    }

    /// Objects left untouched because the destination already matched.
    pub fn skipped(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Skipped))
    }

    /// Objects that disappeared from the source mid-copy.
    pub fn vanished(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Vanished))
    }

    /// Failed objects, plus the prefix itself when it could not be listed.
    pub fn failed(&self) -> usize {
        self.failures().count() + usize::from(self.listing_error.is_some())
    }

    /// Body bytes written for this prefix.
    pub fn bytes(&self) -> u64 {
        self.objects.iter().map(|object| object.bytes).sum()
    }

    /// The failing objects, for the end-of-run detail list.
    pub fn failures(&self) -> impl Iterator<Item = &ObjectReport> {
        self.objects
            .iter()
            .filter(|object| matches!(object.outcome, Outcome::Failed(_)))
    }

    /// Whether the prefix finished with nothing outstanding — the condition
    /// for advancing the resume cursor past it.
    pub fn is_clean(&self) -> bool {
        self.listing_error.is_none() && self.failures().next().is_none()
    }

    fn count(&self, predicate: impl Fn(&Outcome) -> bool) -> usize {
        self.objects
            .iter()
            .filter(|object| predicate(&object.outcome))
            .count()
    }
}

/// Whole-run result.
#[derive(Clone, Debug)]
pub struct CopyReport {
    pub prefixes: Vec<PrefixReport>,
    /// Prefix the resume sentinel fast-forwarded past, empty on a fresh run.
    pub resumed_from: String,
}

impl CopyReport {
    /// Total failed objects across every prefix.
    pub fn failed(&self) -> usize {
        self.prefixes.iter().map(PrefixReport::failed).sum()
    }

    /// Whether every prefix finished with nothing outstanding — the
    /// condition for a zero exit code.
    pub fn is_clean(&self) -> bool {
        self.prefixes.iter().all(PrefixReport::is_clean)
    }

    /// Total body bytes written.
    pub fn bytes(&self) -> u64 {
        self.prefixes.iter().map(PrefixReport::bytes).sum()
    }
}

/// One prefix of a `--dry-run` plan.
#[derive(Clone, Debug)]
pub struct PrefixPlan {
    pub prefix: String,
    /// Objects the source holds under this prefix.
    pub source_objects: usize,
    /// How many of them already exist at the destination (by name).
    pub already_at_destination: usize,
    /// Whether the resume sentinel would fast-forward past this prefix.
    pub fast_forward: bool,
}

/// A `--dry-run` plan: what a real run would touch, having written nothing.
#[derive(Clone, Debug)]
pub struct CopyPlan {
    pub prefixes: Vec<PrefixPlan>,
    pub resumed_from: String,
}

/// Sentinel body: resume cursor plus cumulative counts across runs.
#[derive(Clone, Debug, Default)]
struct Sentinel {
    cursor: String,
    copied: u64,
    repaired: u64,
    skipped: u64,
    vanished: u64,
    failed: u64,
    bytes: u64,
}

/// Read the destination sentinel; a missing, empty or unparseable body
/// starts from scratch (same tolerance as `migrations::read_sentinel`).
async fn read_sentinel(destination: &Arc<dyn BlobBackend>) -> Result<Sentinel, StorageError> {
    let Some(raw) = destination.download_text(SENTINEL_PATH).await? else {
        return Ok(Sentinel::default());
    };
    if raw.is_empty() {
        return Ok(Sentinel::default());
    }
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    let number = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    };
    Ok(Sentinel {
        cursor: value
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        copied: number("copied"),
        repaired: number("repaired"),
        skipped: number("skipped"),
        vanished: number("vanished"),
        failed: number("failed"),
        bytes: number("bytes"),
    })
}

/// Persist the sentinel to the destination (Python-compatible `json.dumps`
/// separators, like every other JSON body this crate writes).
async fn write_sentinel(
    destination: &Arc<dyn BlobBackend>,
    state: &Sentinel,
) -> Result<(), StorageError> {
    let body = super::python_json_dumps(&serde_json::json!({
        "cursor": state.cursor,
        "copied": state.copied,
        "repaired": state.repaired,
        "skipped": state.skipped,
        "vanished": state.vanished,
        "failed": state.failed,
        "bytes": state.bytes,
    }))?;
    destination.upload_text(SENTINEL_PATH, &body).await
}

/// The prefixes a run will walk: the explicit selection, or the canonical
/// set when none was given.
fn selected_prefixes(requested: &[String]) -> Vec<String> {
    if requested.is_empty() {
        return CANONICAL_PREFIXES
            .iter()
            .map(|prefix| (*prefix).to_string())
            .collect();
    }
    requested.to_vec()
}

/// Index a destination listing by object name, with metadata keys folded to
/// lowercase for comparison (see [`metadata_satisfied`]).
fn index_by_name(blobs: Vec<BlobInfo>) -> BTreeMap<String, BTreeMap<String, String>> {
    blobs
        .into_iter()
        .map(|blob| (blob.name, lowercase_keys(&blob.metadata)))
        .collect()
}

fn lowercase_keys(metadata: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .iter()
        .map(|(key, value)| (key.to_lowercase(), value.clone()))
        .collect()
}

/// Whether `landed` already carries everything `wanted` asks for.
///
/// Keys are compared case-insensitively because the two backends disagree
/// on case: Azure round-trips metadata through case-insensitive
/// `x-ms-meta-*` headers, GCS preserves the key exactly as written.
///
/// Empty values are ignored: `<AzureBlobBackend as BlobBackend>::set_metadata`
/// filters empty values out before the PUT, so they can never land and must
/// not be reported as a lost write.
///
/// Extra destination keys are fine — both backends MERGE on `set_metadata`,
/// so the destination is only ever required to be a superset.
fn metadata_satisfied(
    landed: &BTreeMap<String, String>,
    wanted: &BTreeMap<String, String>,
) -> bool {
    wanted
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .all(|(key, value)| landed.get(&key.to_lowercase()) == Some(value))
}

/// Copy one object. Never returns `Err`: a single bad object is recorded in
/// the report so the rest of the run continues and the operator gets the
/// complete list of what needs attention.
async fn copy_object(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    blob: &BlobInfo,
    landed: Option<&BTreeMap<String, String>>,
) -> ObjectReport {
    let name = blob.name.clone();
    let report = |bytes: u64, outcome: Outcome| ObjectReport {
        name: name.clone(),
        bytes,
        outcome,
    };
    let failed = |context: &str, err: StorageError| Outcome::Failed(format!("{context}: {err}"));

    let body = match source.download_bytes(&blob.name).await {
        Ok(Some(body)) => body,
        // Listed a moment ago, gone now: a live queue moved the job.
        Ok(None) => return report(u64::default(), Outcome::Vanished),
        Err(err) => return report(u64::default(), failed("source read failed", err)),
    };
    let size = body.len() as u64;

    // Already there? Compare sizes. BlobBackend exposes no content length,
    // so the destination body is read only for names the destination
    // listing already reported.
    if let Some(landed) = landed {
        let existing = match destination.download_bytes(&blob.name).await {
            Ok(existing) => existing,
            Err(err) => return report(u64::default(), failed("destination read failed", err)),
        };
        if existing.is_some_and(|existing| existing.len() == body.len()) {
            if metadata_satisfied(landed, &blob.metadata) {
                return report(u64::default(), Outcome::Skipped);
            }
            // Body is already right, metadata is not — the exact residue of
            // an earlier run whose Azure metadata PUT was swallowed. Repair
            // the metadata without rewriting the body.
            if let Err(err) = destination.set_metadata(&blob.name, &blob.metadata).await {
                return report(u64::default(), failed("metadata write failed", err));
            }
            return report(u64::default(), Outcome::MetadataRepaired);
        }
    }

    if let Err(err) = destination.upload_bytes(&blob.name, &body).await {
        return report(u64::default(), failed("destination write failed", err));
    }
    if !blob.metadata.is_empty() {
        if let Err(err) = destination.set_metadata(&blob.name, &blob.metadata).await {
            return report(u64::default(), failed("metadata write failed", err));
        }
    }
    report(size, Outcome::Copied)
}

/// Copy every object under one prefix, then verify what landed.
async fn copy_prefix(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    prefix: &str,
    concurrency: usize,
) -> PrefixReport {
    let mut report = PrefixReport {
        prefix: prefix.to_string(),
        listing_error: None,
        objects: Vec::new(),
    };
    let blobs = match source.list_blobs_with_meta(prefix).await {
        Ok(blobs) => blobs,
        Err(err) => {
            report.listing_error = Some(format!("source listing failed: {err}"));
            return report;
        }
    };
    if blobs.is_empty() {
        return report;
    }
    let present = match destination.list_blobs_with_meta(prefix).await {
        Ok(existing) => index_by_name(existing),
        Err(err) => {
            report.listing_error = Some(format!("destination listing failed: {err}"));
            return report;
        }
    };

    // Same fan-out idiom as `migrations::backfill_priority_markers`:
    // `buffered` keeps the results aligned with `blobs`, which the
    // verification pass below relies on.
    report.objects = futures::stream::iter(&blobs)
        .map(|blob| copy_object(source, destination, blob, present.get(&blob.name)))
        .buffered(concurrency)
        .collect::<Vec<ObjectReport>>()
        .await;

    verify_metadata(destination, prefix, &blobs, &mut report).await;
    report
}

/// Re-read the destination prefix and confirm the metadata actually landed.
///
/// This pass is mandatory, not paranoia:
/// `<AzureBlobBackend as BlobBackend>::set_metadata` logs and SWALLOWS both
/// a failed request and a non-success response, returning `Ok` either way
/// (Python parity). A successful `set_metadata` therefore carries no
/// information at all, and the scheduler prefilter in `queue::listing`
/// depends on those keys. One listing per prefix re-reads everything the
/// run just wrote.
async fn verify_metadata(
    destination: &Arc<dyn BlobBackend>,
    prefix: &str,
    blobs: &[BlobInfo],
    report: &mut PrefixReport,
) {
    let wrote = |outcome: &Outcome| matches!(outcome, Outcome::Copied | Outcome::MetadataRepaired);
    if !report.objects.iter().any(|object| wrote(&object.outcome)) {
        return;
    }
    let landed = match destination.list_blobs_with_meta(prefix).await {
        Ok(landed) => index_by_name(landed),
        Err(err) => {
            report.listing_error = Some(format!("metadata verification listing failed: {err}"));
            return;
        }
    };
    // `buffered` preserved order, so the two sequences line up.
    for (blob, object) in blobs.iter().zip(report.objects.iter_mut()) {
        if !wrote(&object.outcome) {
            continue;
        }
        match landed.get(&blob.name) {
            None => {
                object.bytes = u64::default();
                object.outcome = Outcome::Failed(
                    "object is absent from the destination listing after the write".into(),
                );
            }
            Some(found) if !metadata_satisfied(found, &blob.metadata) => {
                object.bytes = u64::default();
                object.outcome = Outcome::Failed(format!(
                    "metadata did not land: wanted {:?}, destination has {found:?}",
                    blob.metadata
                ));
            }
            Some(_) => {}
        }
    }
}

/// Replicate the configured primary store to its disaster-recovery endpoint.
///
/// The coordinator calls this after every dispatch tick. Writes stay
/// single-primary; the backup is never promoted automatically, which avoids
/// split-brain when primary health is uncertain.
/// After a clean copy, stale objects are pruned from canonical backup
/// prefixes so lifecycle moves and deletes remain exact during read failover.
pub async fn replicate_configured_backup() -> Result<Option<CopyReport>, StorageError> {
    let Some(destination_endpoint) = Endpoint::configured_backup() else {
        return Ok(None);
    };
    let source_endpoint = Endpoint::configured_primary();
    if source_endpoint.describe() == destination_endpoint.describe() {
        return Err(StorageError::Other(format!(
            "primary and backup resolve to the same store ({})",
            source_endpoint.describe()
        )));
    }
    let source = source_endpoint.build().await?;
    let destination = destination_endpoint.build().await?;
    let report = copy(
        &source,
        &destination,
        &CopyOptions {
            prefixes: Vec::new(),
            concurrency: DEFAULT_CONCURRENCY,
        },
    )
    .await?;
    if report.is_clean() {
        prune_backup_extras(&source, &destination).await?;
    }
    Ok(Some(report))
}

async fn prune_backup_extras(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
) -> Result<(), StorageError> {
    for prefix in CANONICAL_PREFIXES {
        let source_names = source
            .list_blobs_with_meta(prefix)
            .await?
            .into_iter()
            .map(|blob| blob.name)
            .collect::<BTreeSet<_>>();
        let destination_names = destination
            .list_blobs_with_meta(prefix)
            .await?
            .into_iter()
            .map(|blob| blob.name)
            .collect::<BTreeSet<_>>();
        for stale in destination_names.difference(&source_names) {
            if !source.exists(stale).await? {
                destination.delete(stale).await?;
            }
        }
    }
    Ok(())
}

/// Plan a copy without writing anything: per-prefix source counts and how
/// much of that is already at the destination.
pub async fn plan(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    options: &CopyOptions,
) -> Result<CopyPlan, StorageError> {
    let prefixes = selected_prefixes(&options.prefixes);
    let sentinel = read_sentinel(destination).await?;
    let (resumed_from, remaining) =
        resume_split(&prefixes, &sentinel.cursor, options.prefixes.is_empty());
    // `remaining` is a suffix of `prefixes`, so everything before it is
    // what the resume cursor would fast-forward past.
    let fast_forwarded = prefixes.len() - remaining.len();
    let mut planned = Vec::new();
    for (index, prefix) in prefixes.iter().enumerate() {
        let blobs = source.list_blobs_with_meta(prefix).await?;
        let present = index_by_name(destination.list_blobs_with_meta(prefix).await?);
        planned.push(PrefixPlan {
            prefix: prefix.clone(),
            source_objects: blobs.len(),
            already_at_destination: blobs
                .iter()
                .filter(|blob| present.contains_key(&blob.name))
                .count(),
            fast_forward: index < fast_forwarded,
        });
    }
    Ok(CopyPlan {
        prefixes: planned,
        resumed_from,
    })
}

/// Split the selected prefixes at the resume cursor: the cursor that was
/// consumed (empty when the run starts from the top) and the prefixes still
/// to walk.
///
/// An exhausted tail restarts from the top. The cursor is a within-run
/// resume point, NOT a "done" flag: the pre-cutover re-sync has to walk
/// every prefix again to catch churn, and object-level skipping keeps that
/// cheap.
fn resume_split<'a>(
    prefixes: &'a [String],
    cursor: &str,
    full_run: bool,
) -> (String, &'a [String]) {
    let Some(index) = resume_index(prefixes, cursor, full_run) else {
        return (String::new(), prefixes);
    };
    // `split_first` drops the cursor prefix itself: it finished cleanly.
    match prefixes[index..].split_first() {
        Some((done, rest)) if !rest.is_empty() => (done.clone(), rest),
        _ => (String::new(), prefixes),
    }
}

/// Index of the last prefix the resume cursor covers.
///
/// The cursor is only honored on a full canonical run: when the operator
/// names prefixes explicitly they asked for exactly those, and silently
/// fast-forwarding past them would be a trap. Per-object skipping keeps the
/// restricted re-run cheap anyway.
fn resume_index(prefixes: &[String], cursor: &str, full_run: bool) -> Option<usize> {
    if !full_run || cursor.is_empty() {
        return None;
    }
    prefixes.iter().position(|prefix| prefix == cursor)
}

/// Copy every selected prefix from `source` to `destination`.
///
/// Never deletes. The resume cursor advances only while prefixes finish
/// clean and in order — once one fails, later prefixes are still copied
/// (best effort) but the cursor stops, so the next run retries from the
/// last known-good point.
pub async fn copy(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    options: &CopyOptions,
) -> Result<CopyReport, StorageError> {
    let prefixes = selected_prefixes(&options.prefixes);
    let mut sentinel = read_sentinel(destination).await?;
    let (resumed_from, remaining) =
        resume_split(&prefixes, &sentinel.cursor, options.prefixes.is_empty());

    let mut report = CopyReport {
        prefixes: Vec::new(),
        resumed_from,
    };
    let mut cursor_open = true;
    for prefix in remaining {
        let prefix_report = copy_prefix(source, destination, prefix, options.concurrency).await;
        sentinel.copied = sentinel
            .copied
            .saturating_add(prefix_report.copied() as u64);
        sentinel.repaired = sentinel
            .repaired
            .saturating_add(prefix_report.repaired() as u64);
        sentinel.skipped = sentinel
            .skipped
            .saturating_add(prefix_report.skipped() as u64);
        sentinel.vanished = sentinel
            .vanished
            .saturating_add(prefix_report.vanished() as u64);
        sentinel.failed = sentinel
            .failed
            .saturating_add(prefix_report.failed() as u64);
        sentinel.bytes = sentinel.bytes.saturating_add(prefix_report.bytes());
        cursor_open = cursor_open && prefix_report.is_clean();
        if cursor_open {
            sentinel.cursor = prefix.clone();
        }
        // Persist after every prefix so an interrupted run resumes here.
        write_sentinel(destination, &sentinel).await?;
        report.prefixes.push(prefix_report);
    }
    if cursor_open {
        // Clean run: drop the cursor so the next invocation is a full
        // re-sync rather than a no-op.
        sentinel.cursor = String::new();
        write_sentinel(destination, &sentinel).await?;
    }
    Ok(report)
}
