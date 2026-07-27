//! `stado storage` — the cross-backend copier plus the read-only
//! inspection commands the operator needs when the STORE itself is the
//! suspect.
//!
//! NO Python original: the Python CLI has neither a cross-backend copier
//! (see the module docs of [`crate::queue::copy`] for why) nor any way to
//! look at the raw store. This is the operator surface for both.
//!
//! `copy` and `verify` take every locator as an explicit flag, so the
//! source and the destination are built independently of
//! `WC_STORAGE_BACKEND` and can be two different kinds of store in the
//! same process. `ls` / `stat` / `cat` inspect the ONE configured store
//! and therefore go through [`crate::queue::JobStorage`].
//!
//! # Absent is not unreachable
//!
//! The GCP-billing outage left nobody able to answer "is the queue empty,
//! or is the store gone?", because
//! `<AzureBlobBackend as BlobBackend>::exists` maps EVERY failure to
//! `false` (Python parity: `except Exception: return False`) and
//! `<AzureBlobBackend as BlobBackend>::updated_at` maps every failure to
//! `None`. Through those two methods a forbidden container and an empty
//! one are the same answer. Nothing in this module calls either of them:
//!
//! - `stat` probes with `BlobBackend::download_text_versioned`, which
//!   propagates [`crate::queue::StorageError`]. `absent` (the store
//!   answered, the object is not there) and `unreachable` (the store did
//!   not answer) are different states with different exit codes.
//! - `ls` renders a per-prefix listing failure as `unreachable` and exits
//!   non-zero, instead of folding it into a zero count.
//! - `verify` treats an unreadable side as unknown, never as empty.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::num::NonZeroUsize;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use clap::{Args, Subcommand};
use futures::StreamExt;
use serde_json::{json, Value};

use crate::queue::copy::{
    self, CopyOptions, CopyPlan, CopyReport, Endpoint, Outcome, CANONICAL_PREFIXES,
};
use crate::queue::{BlobBackend, BlobInfo, JobStorage};

use super::table::print as print_table;
use super::CmdError;

#[derive(Subcommand)]
pub enum StorageCommands {
    /// Copy queue state from one storage backend to another.
    Copy(Box<StorageCopyArgs>),
    /// Copy the active queue store to the configured disaster-recovery store.
    Backup(StorageBackupArgs),
    /// List objects under a prefix, or per-prefix counts across the whole
    /// canonical prefix set when no prefix is given.
    Ls(StorageLsArgs),
    /// Report one object: present, absent, or unreachable.
    Stat(StorageStatArgs),
    /// Write one object's body to stdout.
    Cat(StorageCatArgs),
    /// Compare two stores object-for-object. Read-only; copies nothing.
    Verify(Box<StorageVerifyArgs>),
}

/// The locator flags shared by `copy` and `verify`, so both commands
/// address a pair of stores with an identical flag set.
#[derive(Args, Debug)]
pub struct EndpointArgs {
    /// Source backend.
    #[arg(long, value_parser = ["gcs", "azure", "s3", "local"])]
    from: String,
    /// Destination backend.
    #[arg(long, value_parser = ["gcs", "azure", "s3", "local"])]
    to: String,

    /// Source bucket (gcs, s3).
    #[arg(long, default_value = "")]
    from_bucket: String,
    /// Destination bucket (gcs, s3).
    #[arg(long, default_value = "")]
    to_bucket: String,
    /// Source storage account (azure).
    #[arg(long, default_value = "")]
    from_account: String,
    /// Destination storage account (azure).
    #[arg(long, default_value = "")]
    to_account: String,
    /// Source container (azure).
    #[arg(long, default_value = "")]
    from_container: String,
    /// Destination container (azure).
    #[arg(long, default_value = "")]
    to_container: String,
    /// Source root directory (local).
    #[arg(long, default_value = "")]
    from_path: String,
    /// Destination root directory (local).
    #[arg(long, default_value = "")]
    to_path: String,
    /// Source region (s3); empty defers to the AWS default chain.
    #[arg(long, default_value = "")]
    from_region: String,
    /// Destination region (s3); empty defers to the AWS default chain.
    #[arg(long, default_value = "")]
    to_region: String,
}

impl EndpointArgs {
    fn source(&self) -> Endpoint {
        Endpoint {
            kind: self.from.clone(),
            bucket: self.from_bucket.clone(),
            account: self.from_account.clone(),
            container: self.from_container.clone(),
            region: self.from_region.clone(),
            path: self.from_path.clone(),
        }
    }

    fn destination(&self) -> Endpoint {
        Endpoint {
            kind: self.to.clone(),
            bucket: self.to_bucket.clone(),
            account: self.to_account.clone(),
            container: self.to_container.clone(),
            region: self.to_region.clone(),
            path: self.to_path.clone(),
        }
    }
}

#[derive(Args, Debug)]
pub struct StorageCopyArgs {
    #[command(flatten)]
    ends: EndpointArgs,

    /// Restrict the copy to this prefix. Repeatable. Omit to copy the whole
    /// canonical prefix set.
    #[arg(long = "prefix")]
    prefix: Vec<String>,
    /// Print the per-prefix plan and copy nothing.
    #[arg(long)]
    dry_run: bool,
    /// Objects copied in parallel.
    #[arg(long, default_value_t = default_concurrency())]
    concurrency: NonZeroUsize,
}

#[derive(Args, Debug)]
pub struct StorageBackupArgs {
    /// Restrict the backup to this prefix. Repeatable. Omit to copy the
    /// complete canonical state set.
    #[arg(long = "prefix")]
    prefix: Vec<String>,
    /// Print the plan and write nothing.
    #[arg(long)]
    dry_run: bool,
    /// Objects copied in parallel.
    #[arg(long, default_value_t = default_concurrency())]
    concurrency: NonZeroUsize,
}

#[derive(Args, Debug)]
pub struct StorageLsArgs {
    /// Object-name prefix. Omit for per-prefix counts across the canonical
    /// prefix set — the fast answer to "is the queue empty?".
    prefix: Option<String>,
    /// Maximum objects listed under an explicit prefix.
    #[arg(long, default_value_t = default_list_limit())]
    limit: usize,
    /// Also report each listed object's body size. Opt-in because it costs
    /// one download per object; see `probe_sizes`.
    #[arg(long)]
    size: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct StorageStatArgs {
    /// Full object name, for example `queue/<job_id>.json` or
    /// `registry.json`.
    path: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct StorageCatArgs {
    /// Full object name, for example `registry.json`.
    path: String,
}

#[derive(Args, Debug)]
pub struct StorageVerifyArgs {
    #[command(flatten)]
    ends: EndpointArgs,

    /// Restrict the comparison to this prefix. Repeatable. Omit to compare
    /// the whole canonical prefix set.
    #[arg(long = "prefix")]
    prefix: Vec<String>,
    #[arg(long)]
    json: bool,
}

/// [`copy::DEFAULT_CONCURRENCY`] as the non-zero type the flag parses into;
/// `buffered(0)` would make no progress, so zero is rejected at parse time.
fn default_concurrency() -> NonZeroUsize {
    NonZeroUsize::new(copy::DEFAULT_CONCURRENCY).expect("the crate fan-out budget is non-zero")
}

/// Default `--limit` for `storage ls`: the largest count one byte can
/// express. A hot prefix (`queue/` carries five figures of blobs) is
/// truncated with a note rather than flooding the terminal, and the
/// operator raises the flag when they want the rest.
fn default_list_limit() -> usize {
    usize::from(u8::MAX)
}

pub async fn dispatch(command: StorageCommands) -> Result<(), CmdError> {
    match command {
        StorageCommands::Copy(args) => run(&args).await,
        StorageCommands::Backup(args) => backup(&args).await,
        StorageCommands::Ls(args) => ls(&args).await,
        StorageCommands::Stat(args) => stat(&args).await,
        StorageCommands::Cat(args) => cat(&args).await,
        StorageCommands::Verify(args) => verify(&args).await,
    }
}

// ---- copy ----

async fn run(args: &StorageCopyArgs) -> Result<(), CmdError> {
    copy_between(
        args.ends.source(),
        args.ends.destination(),
        CopyOptions {
            prefixes: args.prefix.clone(),
            concurrency: args.concurrency.get(),
        },
        args.dry_run,
    )
    .await
}

async fn backup(args: &StorageBackupArgs) -> Result<(), CmdError> {
    let destination = Endpoint::configured_backup().ok_or_else(|| {
        CmdError::click(
            "no disaster-recovery store is configured; set WC_BACKUP_STORAGE_BACKEND and its locator",
        )
    })?;
    copy_between(
        Endpoint::configured_primary(),
        destination,
        CopyOptions {
            prefixes: args.prefix.clone(),
            concurrency: args.concurrency.get(),
        },
        args.dry_run,
    )
    .await
}

async fn copy_between(
    from: Endpoint,
    to: Endpoint,
    options: CopyOptions,
    dry_run: bool,
) -> Result<(), CmdError> {
    if from.describe() == to.describe() {
        return Err(CmdError::click(format!(
            "source and destination are the same store ({}); nothing to copy",
            from.describe()
        )));
    }

    let source = from.build().await?;
    let destination = to.build().await?;

    println!("{} -> {}", from.describe(), to.describe());
    if dry_run {
        let plan = copy::plan(&source, &destination, &options).await?;
        print_plan(&plan);
        print_split_brain_warning();
        return Ok(());
    }

    let report = copy::copy(&source, &destination, &options).await?;
    print_report(&report);
    print_split_brain_warning();
    if !report.is_clean() {
        return Err(CmdError::click(format!(
            "{} object(s) failed to copy; the resume sentinel at {} was left at the last \
             clean prefix, so re-running continues from there",
            report.failed(),
            copy::SENTINEL_PATH
        )));
    }
    Ok(())
}

fn print_plan(plan: &CopyPlan) {
    println!("DRY RUN — nothing is written.");
    if !plan.resumed_from.is_empty() {
        println!(
            "Resume sentinel {} stops at {:?}; a real run would fast-forward past every \
             prefix up to and including it.",
            copy::SENTINEL_PATH,
            plan.resumed_from
        );
    }
    let rows: Vec<Vec<String>> = plan
        .prefixes
        .iter()
        .map(|row| {
            vec![
                row.prefix.clone(),
                row.source_objects.to_string(),
                row.already_at_destination.to_string(),
                if row.fast_forward {
                    "fast-forward".to_string()
                } else {
                    String::new()
                },
            ]
        })
        .collect();
    print_table(&["PREFIX", "AT SOURCE", "AT DESTINATION", "RESUME"], &rows);
    let total: usize = plan.prefixes.iter().map(|row| row.source_objects).sum();
    println!(
        "\n{total} source object(s) across {} prefix(es).",
        plan.prefixes.len()
    );
}

fn print_report(report: &CopyReport) {
    if !report.resumed_from.is_empty() {
        println!(
            "Resumed after {:?} (sentinel {}).",
            report.resumed_from,
            copy::SENTINEL_PATH
        );
    }
    let rows: Vec<Vec<String>> = report
        .prefixes
        .iter()
        .map(|prefix| {
            vec![
                prefix.prefix.clone(),
                prefix.copied().to_string(),
                prefix.repaired().to_string(),
                prefix.skipped().to_string(),
                prefix.vanished().to_string(),
                prefix.failed().to_string(),
                prefix.bytes().to_string(),
            ]
        })
        .collect();
    print_table(
        &[
            "PREFIX",
            "COPIED",
            "META-FIXED",
            "SKIPPED",
            "VANISHED",
            "FAILED",
            "BYTES",
        ],
        &rows,
    );
    println!("\n{} byte(s) written.", report.bytes());

    let vanished: Vec<&str> = report
        .prefixes
        .iter()
        .flat_map(|prefix| prefix.objects.iter())
        .filter(|object| object.outcome == Outcome::Vanished)
        .map(|object| object.name.as_str())
        .collect();
    if !vanished.is_empty() {
        println!(
            "{} object(s) disappeared from the source mid-copy — the queue is LIVE.",
            vanished.len()
        );
    }

    if report.is_clean() {
        return;
    }
    let failed = report.failed();
    println!("\n{failed} failure(s):");
    for prefix in &report.prefixes {
        if let Some(error) = &prefix.listing_error {
            println!("  {}: {error}", prefix.prefix);
        }
        for object in prefix.failures() {
            if let Outcome::Failed(reason) = &object.outcome {
                println!("  {}: {reason}", object.name);
            }
        }
    }
}

/// The hazard the whole migration turns on. `deploy/MIGRATE_TO_STADO.md`
/// documents it as the reason the copy step is gated on a drained fleet.
fn print_split_brain_warning() {
    println!(
        "\nWARNING: copying a LIVE queue produces split-brain — a job claimed from the old \
         store, written to the new one, and reaped from neither. Drain the fleet first \
         (stop the coordinator tick and every agent, then confirm there are no queued and \
         no running jobs) and copy again immediately before the cutover. \
         See deploy/MIGRATE_TO_STADO.md."
    );
}

// ---- ls ----

async fn ls(args: &StorageLsArgs) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    match args.prefix.as_deref() {
        Some(prefix) => ls_prefix(&store, prefix, args).await,
        None => ls_canonical(&store, args.json).await,
    }
}

/// Per-prefix object counts across [`CANONICAL_PREFIXES`].
///
/// This is the fast operator answer during an outage, and the one place
/// where a listing failure must NOT render as an empty prefix: a store
/// that cannot be listed reports `unreachable` and the command exits
/// non-zero, so "the queue drained" can never be confused with "the queue
/// is behind a 403".
async fn ls_canonical(store: &JobStorage, as_json: bool) -> Result<(), CmdError> {
    let backend = store.backend();
    let counted: Vec<(&str, Result<usize, String>)> = futures::stream::iter(CANONICAL_PREFIXES)
        .map(|prefix| async move {
            let outcome = backend
                .list_blobs_with_meta(prefix)
                .await
                .map(|blobs| blobs.len())
                .map_err(|err| err.to_string());
            (*prefix, outcome)
        })
        .buffered(copy::DEFAULT_CONCURRENCY)
        .collect()
        .await;

    let unreachable: Vec<&str> = counted
        .iter()
        .filter(|(_, outcome)| outcome.is_err())
        .map(|(prefix, _)| *prefix)
        .collect();
    let total: usize = counted
        .iter()
        .filter_map(|(_, outcome)| outcome.as_ref().ok())
        .sum();

    if as_json {
        let rows: Vec<Value> = counted
            .iter()
            .map(|(prefix, outcome)| match outcome {
                Ok(count) => json!({"prefix": prefix, "objects": count, "status": "ok"}),
                Err(error) => json!({
                    "prefix": prefix,
                    "objects": Value::Null,
                    "status": "unreachable",
                    "error": error,
                }),
            })
            .collect();
        echo_json(&json!({
            "backend": store.backend_name(),
            "bucket": store.bucket_name(),
            "prefixes": rows,
            "objects": total,
            "unreachable": unreachable,
        }))?;
    } else {
        let rows: Vec<Vec<String>> = counted
            .iter()
            .map(|(prefix, outcome)| match outcome {
                Ok(count) => vec![(*prefix).to_string(), count.to_string(), "ok".to_string()],
                Err(error) => vec![
                    (*prefix).to_string(),
                    String::new(),
                    format!("UNREACHABLE: {error}"),
                ],
            })
            .collect();
        println!("{} ({})", store.bucket_name(), store.backend_name());
        print_table(&["PREFIX", "OBJECTS", "STATUS"], &rows);
        println!(
            "\n{total} object(s) across {} readable prefix(es).",
            counted.len() - unreachable.len()
        );
    }

    if unreachable.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} prefix(es) could not be listed ({}); those counts are UNKNOWN, not zero",
        unreachable.len(),
        unreachable.join(", ")
    )))
}

/// Objects under one explicit prefix. A listing failure propagates as
/// [`CmdError`] rather than an empty table, for the same reason
/// `ls_canonical` reports `unreachable`.
async fn ls_prefix(store: &JobStorage, prefix: &str, args: &StorageLsArgs) -> Result<(), CmdError> {
    let backend = store.backend();
    let mut blobs = backend.list_blobs_with_meta(prefix).await?;
    blobs.sort_by(|left, right| left.name.cmp(&right.name));
    let total = blobs.len();
    let truncated = total > args.limit;
    blobs.truncate(args.limit);
    let sizes = if args.size {
        probe_sizes(backend, &blobs).await
    } else {
        Vec::new()
    };

    if args.json {
        let objects: Vec<Value> = blobs
            .iter()
            .enumerate()
            .map(|(index, blob)| {
                let probe = sizes.get(index);
                json!({
                    "name": blob.name,
                    "updated": render_optional_stamp(blob.updated),
                    "size": probe.map_or(Value::Null, SizeProbe::value),
                    "size_error": probe.and_then(SizeProbe::error),
                    "metadata": blob.metadata,
                })
            })
            .collect();
        echo_json(&json!({
            "backend": store.backend_name(),
            "bucket": store.bucket_name(),
            "prefix": prefix,
            "limit": args.limit,
            "listed": objects.len(),
            "total": total,
            "truncated": truncated,
            "objects": objects,
        }))?;
        return Ok(());
    }

    let mut headers: Vec<&str> = vec!["NAME", "UPDATED"];
    if args.size {
        headers.push("SIZE");
    }
    headers.push("METADATA");
    let rows: Vec<Vec<String>> = blobs
        .iter()
        .enumerate()
        .map(|(index, blob)| {
            let mut row = vec![blob.name.clone(), render_stamp(blob.updated)];
            if args.size {
                row.push(sizes.get(index).map_or_else(String::new, SizeProbe::cell));
            }
            row.push(render_metadata(&blob.metadata));
            row
        })
        .collect();
    print_table(&headers, &rows);
    println!("\n{} of {total} object(s) under {prefix:?}.", blobs.len());
    if truncated {
        println!(
            "Truncated by --limit {}; raise it to see the rest.",
            args.limit
        );
    }
    Ok(())
}

/// Body length of each listed object.
///
/// Cost note, the same one [`crate::queue::copy`] carries in its module
/// docs: [`BlobInfo`] has no size field — the backend listing contract
/// yields name, timestamp and metadata only — so the only route to a byte
/// count is reading the body. That is why `--size` is opt-in and why it is
/// bounded by `--limit`, and the fan-out is the crate's existing bulk
/// budget (`queue::migrations::BULK_WORKERS`, re-exported as
/// [`copy::DEFAULT_CONCURRENCY`]) rather than a second concurrency style.
async fn probe_sizes(backend: &Arc<dyn BlobBackend>, blobs: &[BlobInfo]) -> Vec<SizeProbe> {
    futures::stream::iter(blobs)
        .map(|blob| async move {
            match backend.download_bytes(&blob.name).await {
                Ok(Some(bytes)) => SizeProbe::Bytes(bytes.len()),
                Ok(None) => SizeProbe::Vanished,
                Err(err) => SizeProbe::Failed(err.to_string()),
            }
        })
        .buffered(copy::DEFAULT_CONCURRENCY)
        .collect()
        .await
}

/// Outcome of one `--size` body read. `Vanished` is a real state on a live
/// queue: the object was listed and then claimed away before the read.
enum SizeProbe {
    Bytes(usize),
    Vanished,
    Failed(String),
}

impl SizeProbe {
    fn cell(&self) -> String {
        match self {
            Self::Bytes(bytes) => bytes.to_string(),
            Self::Vanished => "vanished".to_string(),
            Self::Failed(err) => format!("error: {err}"),
        }
    }

    fn value(&self) -> Value {
        match self {
            Self::Bytes(bytes) => json!(bytes),
            Self::Vanished | Self::Failed(_) => Value::Null,
        }
    }

    fn error(&self) -> Option<String> {
        match self {
            Self::Bytes(_) => None,
            Self::Vanished => Some("vanished between listing and read".to_string()),
            Self::Failed(err) => Some(err.clone()),
        }
    }
}

// ---- stat ----

/// What the store said about one object.
enum Presence {
    /// The store answered and the object is there.
    Present {
        size: usize,
        /// Backend generation / ETag, when the versioned read produced one.
        version: Option<String>,
        /// Why the version is missing, when it is.
        detail: Option<String>,
    },
    /// The store answered and the object is NOT there.
    Absent,
    /// The store did not answer. This is the state `BlobBackend::exists`
    /// cannot express.
    Unreachable(String),
}

/// Existence probe for one object.
///
/// Deliberately NOT `BlobBackend::exists`: the Azure implementation maps
/// every transport failure to `false` (Python parity), so a forbidden
/// container reads exactly like an empty one — the confusion that made the
/// billing outage unreadable. `download_text_versioned` propagates
/// [`crate::queue::StorageError`] instead, and answers existence, size and
/// version token in one round trip.
async fn probe(backend: &Arc<dyn BlobBackend>, path: &str) -> Presence {
    match backend.download_text_versioned(path).await {
        Ok(Some(found)) => Presence::Present {
            size: found.content.len(),
            version: Some(found.version),
            detail: None,
        },
        Ok(None) => Presence::Absent,
        // A non-UTF-8 body (a collected artifact) fails the versioned TEXT
        // read without the store being unreachable, so re-probe with the
        // binary read before calling it a transport failure. On a genuinely
        // unreachable store this second probe fails too and costs one
        // request.
        Err(err) => match backend.download_bytes(path).await {
            Ok(Some(bytes)) => Presence::Present {
                size: bytes.len(),
                version: None,
                detail: Some(format!("no version token: {err}")),
            },
            Ok(None) => Presence::Absent,
            Err(_) => Presence::Unreachable(err.to_string()),
        },
    }
}

/// Exit code contract: zero means the question was ANSWERED (`present` or
/// `absent`), non-zero means it was not (`unreachable`). Scripting an
/// "is it gone?" check on the exit status therefore never mistakes a dead
/// store for a drained one; branch on `state` for present-vs-absent.
async fn stat(args: &StorageStatArgs) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let backend = store.backend();
    let presence = probe(backend, &args.path).await;

    // Metadata and the timestamp come from the listing, which is a separate
    // grant from object read: if listing is denied while the read worked,
    // say so rather than downgrading a known-present object to unreachable.
    let (metadata, updated, metadata_error) = match &presence {
        Presence::Present { .. } => match backend.list_blobs_with_meta(&args.path).await {
            Ok(blobs) => blobs
                .into_iter()
                .find(|blob| blob.name == args.path)
                .map_or_else(
                    || (BTreeMap::new(), None, None),
                    |blob| (blob.metadata, blob.updated, None),
                ),
            Err(err) => (BTreeMap::new(), None, Some(err.to_string())),
        },
        Presence::Absent | Presence::Unreachable(_) => (BTreeMap::new(), None, None),
    };

    let (state, size, version, detail) = match &presence {
        Presence::Present {
            size,
            version,
            detail,
        } => ("present", Some(*size), version.clone(), detail.clone()),
        Presence::Absent => ("absent", None, None, None),
        Presence::Unreachable(err) => ("unreachable", None, None, Some(err.clone())),
    };

    if args.json {
        echo_json(&json!({
            "backend": store.backend_name(),
            "bucket": store.bucket_name(),
            "path": args.path,
            "state": state,
            "size": size,
            "updated_at": render_optional_stamp(updated),
            "version": version,
            "metadata": metadata,
            "detail": detail,
            "metadata_error": metadata_error,
        }))?;
    } else {
        let mut rows = vec![
            vec!["path".to_string(), args.path.clone()],
            vec!["state".to_string(), state.to_string()],
            vec![
                "store".to_string(),
                format!("{} ({})", store.bucket_name(), store.backend_name()),
            ],
        ];
        if let Some(size) = size {
            rows.push(vec!["size".to_string(), size.to_string()]);
        }
        rows.push(vec!["updated_at".to_string(), render_stamp(updated)]);
        rows.push(vec!["version".to_string(), version.unwrap_or_default()]);
        rows.push(vec!["metadata".to_string(), render_metadata(&metadata)]);
        if let Some(detail) = &detail {
            rows.push(vec!["detail".to_string(), detail.clone()]);
        }
        if let Some(error) = &metadata_error {
            rows.push(vec!["metadata_error".to_string(), error.clone()]);
        }
        print_table(&["FIELD", "VALUE"], &rows);
        if matches!(presence, Presence::Absent) {
            println!(
                "\nThe store ANSWERED: {:?} is not there. This is not the same as an \
                 unreachable store.",
                args.path
            );
        }
    }

    match presence {
        Presence::Present { .. } | Presence::Absent => Ok(()),
        Presence::Unreachable(err) => Err(CmdError::click(format!(
            "{:?} is UNREACHABLE, not absent — the store did not answer: {err}. \
             Treat the object's existence as unknown.",
            args.path
        ))),
    }
}

// ---- cat ----

/// Stream one object's body to stdout. Job documents, `registry.json` and
/// the health beacons are all JSON blobs an operator reads directly.
///
/// `read_bytes` propagates [`crate::queue::StorageError`], so an
/// unreachable store is an error here too and never an empty body.
async fn cat(args: &StorageCatArgs) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let Some(bytes) = store.read_bytes(&args.path).await? else {
        return Err(CmdError::click(format!(
            "{:?}: absent — the store answered and the object is not there",
            args.path
        )));
    };
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}

// ---- verify ----

/// How one prefix compares across the two stores.
#[derive(Default)]
struct PrefixDiff {
    prefix: String,
    /// `None` when that side could not be listed: unknown, not empty.
    source_objects: Option<usize>,
    destination_objects: Option<usize>,
    missing: Vec<String>,
    extra: Vec<String>,
    metadata_gaps: Vec<(String, Vec<String>)>,
    source_error: Option<String>,
    destination_error: Option<String>,
}

impl PrefixDiff {
    fn diverged(&self) -> bool {
        self.source_error.is_some()
            || self.destination_error.is_some()
            || !self.missing.is_empty()
            || !self.extra.is_empty()
            || !self.metadata_gaps.is_empty()
    }

    fn status(&self) -> String {
        if let Some(error) = &self.source_error {
            return format!("SOURCE UNREADABLE: {error}");
        }
        if let Some(error) = &self.destination_error {
            return format!("DESTINATION UNREADABLE: {error}");
        }
        if self.diverged() {
            return "DIVERGED".to_string();
        }
        "match".to_string()
    }
}

/// Compare one prefix. Read-only: this lists both ends and downloads
/// nothing, so it is safe against a store the operator is unsure about.
async fn diff_prefix(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    prefix: &str,
) -> PrefixDiff {
    let mut diff = PrefixDiff {
        prefix: prefix.to_string(),
        ..PrefixDiff::default()
    };
    let (listed_source, listed_destination) = tokio::join!(
        source.list_blobs_with_meta(prefix),
        destination.list_blobs_with_meta(prefix),
    );
    let source_blobs = match listed_source {
        Ok(blobs) => blobs,
        Err(err) => {
            diff.source_error = Some(err.to_string());
            Vec::new()
        }
    };
    let destination_blobs = match listed_destination {
        Ok(blobs) => blobs,
        Err(err) => {
            diff.destination_error = Some(err.to_string());
            Vec::new()
        }
    };
    if diff.source_error.is_some() || diff.destination_error.is_some() {
        // Counts stay None on purpose: an unreadable side is unknown.
        return diff;
    }

    diff.source_objects = Some(source_blobs.len());
    diff.destination_objects = Some(destination_blobs.len());
    let landed: BTreeMap<String, BTreeMap<String, String>> = destination_blobs
        .into_iter()
        .map(|blob| (blob.name, lowercase_keys(&blob.metadata)))
        .collect();

    let mut source_names: BTreeSet<String> = BTreeSet::new();
    for blob in &source_blobs {
        source_names.insert(blob.name.clone());
        match landed.get(&blob.name) {
            None => diff.missing.push(blob.name.clone()),
            Some(present) => {
                let gaps = metadata_gaps(present, &lowercase_keys(&blob.metadata));
                if !gaps.is_empty() {
                    diff.metadata_gaps.push((blob.name.clone(), gaps));
                }
            }
        }
    }
    diff.extra = landed
        .keys()
        .filter(|name| !source_names.contains(*name))
        .cloned()
        .collect();
    diff
}

/// Keys `wanted` carries that `landed` does not satisfy.
///
/// This is the rule `queue/copy.rs::metadata_satisfied` enforces after a
/// copy, restated here because that helper is private to the copier and
/// `queue/copy.rs` is unchanged by this command:
///
/// - keys are folded to lowercase, because Azure round-trips metadata
///   through case-insensitive `x-ms-meta-*` headers while GCS preserves the
///   key exactly as written;
/// - empty values are ignored, because
///   `<AzureBlobBackend as BlobBackend>::set_metadata` filters them out
///   before the PUT and they can therefore never land;
/// - extra destination keys are fine, because both backends MERGE on
///   `set_metadata`, so the destination only has to be a superset.
fn metadata_gaps(
    landed: &BTreeMap<String, String>,
    wanted: &BTreeMap<String, String>,
) -> Vec<String> {
    wanted
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .filter(|(key, value)| landed.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect()
}

fn lowercase_keys(metadata: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .iter()
        .map(|(key, value)| (key.to_lowercase(), value.clone()))
        .collect()
}

/// The prefixes a comparison walks: the explicit selection, or the
/// canonical set when none was given. Mirrors
/// `queue/copy.rs::selected_prefixes` so `storage verify` covers exactly
/// what `storage copy` moves.
fn selected_prefixes(requested: &[String]) -> Vec<String> {
    if requested.is_empty() {
        return CANONICAL_PREFIXES
            .iter()
            .map(|prefix| (*prefix).to_string())
            .collect();
    }
    requested.to_vec()
}

/// The post-copy check `deploy/MIGRATE_TO_STADO.md` demands ("verify object
/// counts match") and never provided. Reads both stores and writes to
/// neither; exits non-zero on any divergence.
async fn verify(args: &StorageVerifyArgs) -> Result<(), CmdError> {
    let from = args.ends.source();
    let to = args.ends.destination();
    if from.describe() == to.describe() {
        return Err(CmdError::click(format!(
            "source and destination are the same store ({}); there is nothing to compare",
            from.describe()
        )));
    }
    let source = from.build().await?;
    let destination = to.build().await?;
    let prefixes = selected_prefixes(&args.prefix);

    let diffs: Vec<PrefixDiff> = futures::stream::iter(prefixes.iter())
        .map(|prefix| diff_prefix(&source, &destination, prefix))
        .buffered(copy::DEFAULT_CONCURRENCY)
        .collect()
        .await;

    let missing: usize = diffs.iter().map(|diff| diff.missing.len()).sum();
    let extra: usize = diffs.iter().map(|diff| diff.extra.len()).sum();
    let gaps: usize = diffs.iter().map(|diff| diff.metadata_gaps.len()).sum();
    let diverging = diffs.iter().filter(|diff| diff.diverged()).count();
    let divergent = diffs.iter().any(PrefixDiff::diverged);

    if args.json {
        echo_json(&json!({
            "from": from.describe(),
            "to": to.describe(),
            "prefixes": diffs.iter().map(diff_json).collect::<Vec<Value>>(),
            "missing_at_destination": missing,
            "only_at_destination": extra,
            "metadata_mismatches": gaps,
            "diverging_prefixes": diverging,
            "divergent": divergent,
        }))?;
    } else {
        println!(
            "{} -> {} (read-only; nothing is written)",
            from.describe(),
            to.describe()
        );
        print_diff_table(&diffs);
        print_diff_detail(&diffs);
    }

    if !divergent {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{diverging} of {} prefix(es) diverge: {missing} object(s) missing at the \
         destination, {extra} only at the destination, {gaps} whose metadata did not \
         land. Nothing was copied — re-run `stado storage copy` with the same locators, \
         then verify again.",
        diffs.len()
    )))
}

fn diff_json(diff: &PrefixDiff) -> Value {
    json!({
        "prefix": diff.prefix,
        "source_objects": diff.source_objects,
        "destination_objects": diff.destination_objects,
        "missing_at_destination": diff.missing,
        "only_at_destination": diff.extra,
        "metadata_mismatches": diff
            .metadata_gaps
            .iter()
            .map(|(name, keys)| json!({"name": name, "keys": keys}))
            .collect::<Vec<Value>>(),
        "source_error": diff.source_error,
        "destination_error": diff.destination_error,
        "diverged": diff.diverged(),
    })
}

fn print_diff_table(diffs: &[PrefixDiff]) {
    let rows: Vec<Vec<String>> = diffs
        .iter()
        .map(|diff| {
            vec![
                diff.prefix.clone(),
                render_count(diff.source_objects),
                render_count(diff.destination_objects),
                diff.missing.len().to_string(),
                diff.extra.len().to_string(),
                diff.metadata_gaps.len().to_string(),
                diff.status(),
            ]
        })
        .collect();
    print_table(
        &[
            "PREFIX",
            "AT SOURCE",
            "AT DESTINATION",
            "MISSING",
            "EXTRA",
            "META-DIFF",
            "STATUS",
        ],
        &rows,
    );
}

/// Name every divergent object. An operator mid-cutover needs the list, not
/// a tally, and the list is what tells them whether the gap is churn or a
/// dropped prefix.
fn print_diff_detail(diffs: &[PrefixDiff]) {
    for diff in diffs.iter().filter(|diff| diff.diverged()) {
        println!("\n{}:", diff.prefix);
        if let Some(error) = &diff.source_error {
            println!("  source could not be listed: {error}");
        }
        if let Some(error) = &diff.destination_error {
            println!("  destination could not be listed: {error}");
        }
        for name in &diff.missing {
            println!("  missing at destination: {name}");
        }
        for name in &diff.extra {
            println!("  only at destination: {name}");
        }
        for (name, keys) in &diff.metadata_gaps {
            println!("  metadata did not land on {name}: {}", keys.join(", "));
        }
    }
}

// ---- shared rendering ----

/// Pretty-printed JSON on stdout, same shape as `cli/quota.rs::echo_json`.
/// No Python original to match: none of these commands exist there.
fn echo_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn render_stamp(updated: Option<DateTime<Utc>>) -> String {
    updated.map_or_else(String::new, |stamp| {
        stamp.to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

fn render_optional_stamp(updated: Option<DateTime<Utc>>) -> Value {
    updated.map_or(Value::Null, |stamp| {
        json!(stamp.to_rfc3339_opts(SecondsFormat::Secs, true))
    })
}

fn render_metadata(metadata: &BTreeMap<String, String>) -> String {
    metadata
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<String>>()
        .join(",")
}

/// An unknown count renders as `?`, never as zero.
fn render_count(count: Option<usize>) -> String {
    count.map_or_else(|| "?".to_string(), |count| count.to_string())
}
