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
use std::io::{Read, Write};
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
    /// Upload a product object through the provider-neutral Stado namespace.
    /// Release objects are always create-only, even without --if-absent.
    Put(StoragePutArgs),
    /// Download a product object through the provider-neutral Stado namespace.
    Get(StorageGetArgs),
    /// List product objects in one provider-neutral Stado namespace.
    Objects(StorageObjectsArgs),
    /// Delete a product object through the provider-neutral Stado namespace.
    /// Release objects are immutable and cannot be deleted.
    Rm(StorageRmArgs),
    /// Print the gateway URL; only stado://releases/... is bearer-free.
    Url(StorageUrlArgs),
}

/// The locator flags shared by `copy` and `verify`, so both commands
/// address a pair of stores with an identical flag set.
#[derive(Args, Debug)]
pub struct EndpointArgs {
    /// Source backend.
    #[arg(long, value_parser = ["gcs", "azure", "s3", "local"])]
    pub(crate) from: String,
    /// Destination backend.
    #[arg(long, value_parser = ["gcs", "azure", "s3", "local"])]
    pub(crate) to: String,

    /// Source bucket (gcs, s3).
    #[arg(long, default_value = "")]
    pub(crate) from_bucket: String,
    /// Destination bucket (gcs, s3).
    #[arg(long, default_value = "")]
    pub(crate) to_bucket: String,
    /// Source storage account (azure).
    #[arg(long, default_value = "")]
    pub(crate) from_account: String,
    /// Destination storage account (azure).
    #[arg(long, default_value = "")]
    pub(crate) to_account: String,
    /// Source container (azure).
    #[arg(long, default_value = "")]
    pub(crate) from_container: String,
    /// Destination container (azure).
    #[arg(long, default_value = "")]
    pub(crate) to_container: String,
    /// Source root directory (local).
    #[arg(long, default_value = "")]
    pub(crate) from_path: String,
    /// Destination root directory (local).
    #[arg(long, default_value = "")]
    pub(crate) to_path: String,
    /// Source region (s3); empty defers to the AWS default chain.
    #[arg(long, default_value = "")]
    pub(crate) from_region: String,
    /// Destination region (s3); empty defers to the AWS default chain.
    #[arg(long, default_value = "")]
    pub(crate) to_region: String,
}

impl EndpointArgs {
    pub(crate) fn source(&self) -> Endpoint {
        Endpoint {
            kind: self.from.clone(),
            bucket: self.from_bucket.clone(),
            account: self.from_account.clone(),
            container: self.from_container.clone(),
            region: self.from_region.clone(),
            path: self.from_path.clone(),
        }
    }

    pub(crate) fn destination(&self) -> Endpoint {
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
pub struct StoragePutArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    /// Local source file, or '-' for stdin.
    source: String,
    /// Refuse to replace an existing object. Implied for stado://releases/...
    /// because release objects are immutable.
    #[arg(long)]
    if_absent: bool,
    /// Media type retained as provider-neutral object metadata.
    #[arg(long, default_value = "application/octet-stream")]
    content_type: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct StorageGetArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    /// Local destination file, or '-' for stdout.
    destination: String,
}

#[derive(Args, Debug)]
pub struct StorageObjectsArgs {
    /// Logical namespace, for example `images` or `checkpoints`.
    namespace: String,
    /// Optional key prefix inside the namespace.
    #[arg(default_value = "")]
    prefix: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct StorageRmArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct StorageUrlArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    #[arg(long)]
    json: bool,
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
        StorageCommands::Put(args) => put(&args).await,
        StorageCommands::Get(args) => get(&args).await,
        StorageCommands::Objects(args) => objects(&args).await,
        StorageCommands::Rm(args) => rm(&args).await,
        StorageCommands::Url(args) => object_url(&args),
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
        true,
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
        true,
    )
    .await
}

pub(crate) async fn copy_between(
    from: Endpoint,
    to: Endpoint,
    options: CopyOptions,
    dry_run: bool,
    warn_live: bool,
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
        if warn_live {
            print_split_brain_warning();
        }
        return Ok(());
    }

    let report = copy::copy(&source, &destination, &options).await?;
    print_report(&report);
    if warn_live {
        print_split_brain_warning();
    }
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

// ---- provider-neutral product objects ----

fn max_object_api_error_body() -> usize {
    usize::from(u16::MAX)
}

fn max_object_api_json_body() -> usize {
    max_object_api_error_body() * u8::BITS as usize * u8::BITS as usize * u16::BITS as usize
}

fn max_object_api_download_body() -> usize {
    crate::object_store::max_object_bytes()
}

struct RemoteObjectApi {
    http: reqwest::Client,
    base_url: url::Url,
    token: String,
}

#[derive(serde::Deserialize)]
struct RemotePutResponse {
    state: String,
    uri: String,
    content_type: String,
}

#[derive(serde::Deserialize)]
struct RemoteDeleteResponse {
    state: String,
    uri: String,
}

#[derive(serde::Deserialize)]
struct RemoteObjectListResponse {
    objects: Vec<RemoteObjectListItem>,
}

#[derive(serde::Deserialize)]
struct RemoteObjectListItem {
    uri: String,
    namespace: String,
    key: String,
    size: Option<u64>,
    updated_at: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

impl RemoteObjectApi {
    fn configured() -> Result<Option<Self>, CmdError> {
        let Some(base_url) = configured_object_base_url("STADO_API_URL")? else {
            return Ok(None);
        };
        let token = match std::env::var("STADO_API_TOKEN") {
            Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
            Ok(_) | Err(std::env::VarError::NotPresent) => {
                return Err(CmdError::click(
                    "STADO_API_TOKEN is required when STADO_API_URL is configured",
                ));
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(CmdError::click(
                    "STADO_API_TOKEN must be valid Unicode when STADO_API_URL is configured",
                ));
            }
        };
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Some(Self {
            http,
            base_url,
            token,
        }))
    }

    fn configured_release_reader() -> Result<Option<Self>, CmdError> {
        let Some(base_url) = configured_object_base_url("STADO_API_URL")? else {
            return Ok(None);
        };
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Some(Self {
            http,
            base_url,
            token: String::new(),
        }))
    }

    fn endpoint(&self, route: &str, query: &[(&str, &str)]) -> Result<url::Url, CmdError> {
        object_api_endpoint(&self.base_url, route, query)
    }

    fn request(&self, method: reqwest::Method, endpoint: url::Url) -> reqwest::RequestBuilder {
        let request = self.http.request(method, endpoint);
        if self.token.is_empty() {
            request
        } else {
            request.bearer_auth(&self.token)
        }
    }

    async fn put(
        &self,
        uri: &str,
        content_type: &str,
        if_absent: bool,
        bytes: Vec<u8>,
    ) -> Result<(), CmdError> {
        let if_absent = if if_absent { "true" } else { "false" };
        let endpoint = self.endpoint("/api/object", &[("uri", uri), ("if_absent", if_absent)])?;
        let response = self
            .request(reqwest::Method::PUT, endpoint)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await?;
        let payload: RemotePutResponse = self.response_json(response, "object PUT").await?;
        if payload.state != "stored" || payload.uri != uri || payload.content_type != content_type {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object PUT response",
            ));
        }
        Ok(())
    }

    async fn get(&self, uri: &str) -> Result<Vec<u8>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri)])?;
        let response = self.request(reqwest::Method::GET, endpoint).send().await?;
        self.success_body(response, max_object_api_download_body(), "object GET")
            .await
    }

    async fn get_release(&self, uri: &str) -> Result<Vec<u8>, CmdError> {
        let endpoint = self.endpoint("/api/release/object", &[("uri", uri)])?;
        let response = self.request(reqwest::Method::GET, endpoint).send().await?;
        self.success_body(response, max_object_api_download_body(), "release GET")
            .await
    }

    async fn list(&self, namespace: &str, prefix: &str) -> Result<Vec<Value>, CmdError> {
        let endpoint = self.endpoint(
            "/api/object/list",
            &[("namespace", namespace), ("prefix", prefix)],
        )?;
        let response = self.request(reqwest::Method::GET, endpoint).send().await?;
        let payload: RemoteObjectListResponse = self.response_json(response, "object list").await?;
        let mut values = Vec::with_capacity(payload.objects.len());
        for item in payload.objects {
            let object = crate::object_store::ObjectRef::parse(&item.uri).map_err(|error| {
                CmdError::click(format!(
                    "Stado object API returned an invalid object-list URI: {error}"
                ))
            })?;
            if object.namespace() != namespace
                || !object.key().starts_with(prefix)
                || item.namespace.as_str() != object.namespace()
                || item.key.as_str() != object.key()
            {
                return Err(CmdError::click(
                    "Stado object API returned an inconsistent object-list item",
                ));
            }
            values.push(json!({
                "uri": item.uri,
                "namespace": item.namespace,
                "key": item.key,
                "size": item.size,
                "updated_at": item.updated_at,
                "metadata": item.metadata,
            }));
        }
        Ok(values)
    }

    async fn delete(&self, uri: &str) -> Result<(), CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri)])?;
        let response = self
            .request(reqwest::Method::DELETE, endpoint)
            .send()
            .await?;
        let payload: RemoteDeleteResponse = self.response_json(response, "object DELETE").await?;
        if payload.state != "absent" || payload.uri != uri {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object DELETE response",
            ));
        }
        Ok(())
    }

    async fn response_json<T>(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> Result<T, CmdError>
    where
        T: serde::de::DeserializeOwned,
    {
        let status = response.status();
        let body = self
            .success_body(response, max_object_api_json_body(), operation)
            .await?;
        serde_json::from_slice(&body).map_err(|error| {
            CmdError::click(format!(
                "Stado object API returned invalid JSON for {operation} (HTTP {status}): {error}"
            ))
        })
    }

    async fn success_body(
        &self,
        mut response: reqwest::Response,
        limit: usize,
        operation: &str,
    ) -> Result<Vec<u8>, CmdError> {
        if !response.status().is_success() {
            return Err(self.response_error(response).await);
        }
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(CmdError::click(format!(
                "Stado object API {operation} response exceeds the {limit}-byte limit"
            )));
        }
        let capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default();
        let mut body = Vec::with_capacity(capacity);
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > limit.saturating_sub(body.len()) {
                return Err(CmdError::click(format!(
                    "Stado object API {operation} response exceeds the {limit}-byte limit"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn response_error(&self, mut response: reqwest::Response) -> CmdError {
        let status = response.status();
        let declared_length = response.content_length();
        let max_body = max_object_api_error_body();
        let mut body = Vec::new();
        let mut truncated = false;
        while body.len() < max_body {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => {
                    let detail = response_body_detail(&body, &self.token);
                    return CmdError::click(format!(
                        "Stado object API returned HTTP {status}; partial response body: \
                         {detail}; body read failed: {error}"
                    ));
                }
            };
            let remaining = max_body - body.len();
            if chunk.len() > remaining {
                body.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
        }
        if body.len() == max_body
            && declared_length
                .map(|length| length > max_body as u64)
                .unwrap_or(true)
        {
            truncated = true;
        }
        let detail = response_body_detail(&body, &self.token);
        let suffix = if truncated {
            " [response body truncated]"
        } else {
            ""
        };
        CmdError::click(format!(
            "Stado object API returned HTTP {status}: {detail}{suffix}"
        ))
    }
}

fn configured_object_base_url(variable: &str) -> Result<Option<url::Url>, CmdError> {
    let value = match std::env::var(variable) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(CmdError::click(format!("{variable} must be valid Unicode")));
        }
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let url = url::Url::parse(value)
        .map_err(|error| CmdError::click(format!("invalid {variable}: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(CmdError::click(format!(
            "{variable} must be an absolute HTTP or HTTPS URL"
        )));
    }
    let host = url.host_str().unwrap_or_default();
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !loopback {
        return Err(CmdError::click(format!(
            "{variable} must use HTTPS unless its host is loopback"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CmdError::click(format!(
            "{variable} must not contain embedded credentials"
        )));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(CmdError::click(format!(
            "{variable} must not contain a query string or fragment"
        )));
    }
    Ok(Some(url))
}

fn object_api_endpoint(
    base_url: &url::Url,
    route: &str,
    query: &[(&str, &str)],
) -> Result<url::Url, CmdError> {
    let mut endpoint = base_url.clone();
    {
        let mut segments = endpoint.path_segments_mut().map_err(|()| {
            CmdError::click("configured object base URL cannot be used as an HTTP API base URL")
        })?;
        segments.pop_if_empty();
        for segment in route.trim_start_matches('/').split('/') {
            segments.push(segment);
        }
    }
    if !query.is_empty() {
        let mut pairs = endpoint.query_pairs_mut();
        for &(name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    Ok(endpoint)
}

fn response_body_detail(body: &[u8], secret: &str) -> String {
    let detail = String::from_utf8_lossy(body);
    let detail = detail.trim();
    if detail.is_empty() {
        "<empty response body>".to_string()
    } else if secret.is_empty() {
        detail.to_string()
    } else {
        detail.replace(secret, "[REDACTED]")
    }
}

fn read_object_source(source: &str) -> Result<Vec<u8>, CmdError> {
    if source == "-" {
        let mut bytes = Vec::new();
        std::io::stdin().lock().read_to_end(&mut bytes)?;
        Ok(bytes)
    } else {
        Ok(std::fs::read(source)?)
    }
}

async fn put(args: &StoragePutArgs) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(&args.uri)?;
    let uri = object.to_string();
    let create_only = args.if_absent || object.namespace() == "releases";
    if let Some(remote) = RemoteObjectApi::configured()? {
        let bytes = read_object_source(&args.source)?;
        remote
            .put(&uri, &args.content_type, create_only, bytes)
            .await?;
    } else {
        let path = object.storage_path();
        let store = JobStorage::new().await?;
        let uploaded = if create_only {
            if args.source == "-" {
                let bytes = read_object_source(&args.source)?;
                let mut source = tempfile::NamedTempFile::new()?;
                source.write_all(&bytes)?;
                store.upload_file_if_absent(&path, source.path()).await?
            } else {
                store
                    .upload_file_if_absent(&path, std::path::Path::new(&args.source))
                    .await?
            }
        } else {
            let bytes = read_object_source(&args.source)?;
            store.upload_bytes(&path, &bytes).await?;
            true
        };

        if !uploaded {
            let policy = if object.namespace() == "releases" {
                "release objects are immutable"
            } else {
                "--if-absent refused to replace it"
            };
            return Err(CmdError::click(format!(
                "{object} already exists; {policy}"
            )));
        }

        let metadata = crate::object_store::metadata(&object, &args.content_type);
        store.backend().set_metadata(&path, &metadata).await?;
    }

    if args.json {
        echo_json(&json!({
            "state": "stored",
            "uri": uri,
            "content_type": args.content_type,
        }))?;
    } else {
        println!("{uri}");
    }
    Ok(())
}

async fn get(args: &StorageGetArgs) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(&args.uri)?;
    let uri = object.to_string();
    let bytes = if object.namespace() == "releases" {
        if let Some(remote) = RemoteObjectApi::configured_release_reader()? {
            remote.get_release(&uri).await?
        } else {
            let store = JobStorage::new().await?;
            let Some(bytes) = store.read_bytes(&object.storage_path()).await? else {
                return Err(CmdError::click(format!("{object}: absent")));
            };
            bytes
        }
    } else if let Some(remote) = RemoteObjectApi::configured()? {
        remote.get(&uri).await?
    } else {
        let store = JobStorage::new().await?;
        let Some(bytes) = store.read_bytes(&object.storage_path()).await? else {
            return Err(CmdError::click(format!("{object}: absent")));
        };
        bytes
    };
    if args.destination == "-" {
        let mut out = std::io::stdout().lock();
        out.write_all(&bytes)?;
        out.flush()?;
    } else {
        std::fs::write(&args.destination, bytes)?;
    }
    Ok(())
}

async fn objects(args: &StorageObjectsArgs) -> Result<(), CmdError> {
    let storage_prefix =
        crate::object_store::ObjectRef::namespace_prefix(&args.namespace, &args.prefix)?;
    let values = if let Some(remote) = RemoteObjectApi::configured()? {
        remote.list(&args.namespace, &args.prefix).await?
    } else {
        let store = JobStorage::new().await?;
        let blobs = store
            .backend()
            .list_blobs_with_meta(&storage_prefix)
            .await?;
        let mut values = Vec::with_capacity(blobs.len());
        for blob in blobs {
            let object = crate::object_store::ObjectRef::from_storage_path(&blob.name)?;
            values.push(json!({
                "uri": object.to_string(),
                "namespace": object.namespace(),
                "key": object.key(),
                "size": blob.size,
                "updated_at": render_optional_stamp(blob.updated),
                "metadata": blob.metadata,
            }));
        }
        values
    };
    if args.json {
        echo_json(&json!({"objects": values}))?;
    } else {
        let rows = values
            .iter()
            .map(|value| {
                vec![
                    value["uri"].as_str().unwrap_or_default().to_string(),
                    value
                        .get("size")
                        .and_then(Value::as_u64)
                        .map_or_else(|| "?".to_string(), |size| size.to_string()),
                    value["updated_at"].as_str().unwrap_or_default().to_string(),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["URI", "BYTES", "UPDATED"], &rows);
    }
    Ok(())
}

async fn rm(args: &StorageRmArgs) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(&args.uri)?;
    if object.namespace() == "releases" {
        return Err(CmdError::click(
            "release objects are immutable and cannot be deleted",
        ));
    }
    let uri = object.to_string();
    if let Some(remote) = RemoteObjectApi::configured()? {
        remote.delete(&uri).await?;
    } else {
        let store = JobStorage::new().await?;
        store.delete_blob(&object.storage_path()).await?;
    }
    if args.json {
        echo_json(&json!({"state": "absent", "uri": uri}))?;
    } else {
        println!("{uri}");
    }
    Ok(())
}

fn object_url(args: &StorageUrlArgs) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(&args.uri)?;
    let (base_url, route) = if object.namespace() == "releases" {
        let base_url = configured_object_base_url("STADO_API_URL")?.ok_or_else(|| {
            CmdError::click("STADO_API_URL is required to render a release object URL")
        })?;
        (base_url, "/api/release/object")
    } else {
        let remote = RemoteObjectApi::configured()?.ok_or_else(|| {
            CmdError::click("STADO_API_URL is required to render a private object URL")
        })?;
        (remote.base_url, "/api/object")
    };
    let uri = object.to_string();
    let url = object_api_endpoint(&base_url, route, &[("uri", &uri)])?;
    if args.json {
        echo_json(&json!({"uri": uri, "url": url.as_str()}))?;
    } else {
        println!("{url}");
    }
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
    body_mismatches: Vec<String>,
    body_errors: Vec<(String, String)>,
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
            || !self.body_mismatches.is_empty()
            || !self.body_errors.is_empty()
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

/// Result of comparing one common object's body bytes.
struct BodyCheck {
    name: String,
    mismatch: bool,
    error: Option<String>,
}

async fn compare_body(
    source: Arc<dyn BlobBackend>,
    destination: Arc<dyn BlobBackend>,
    name: String,
) -> BodyCheck {
    let (source_body, destination_body) = tokio::join!(
        source.download_bytes(&name),
        destination.download_bytes(&name),
    );
    let outcome = match (source_body, destination_body) {
        (Ok(Some(source_body)), Ok(Some(destination_body))) => {
            return BodyCheck {
                name,
                mismatch: source_body != destination_body,
                error: None,
            };
        }
        (Ok(None), _) => "source object vanished after listing".to_string(),
        (_, Ok(None)) => "destination object vanished after listing".to_string(),
        (Err(error), _) => format!("source body read failed: {error}"),
        (_, Err(error)) => format!("destination body read failed: {error}"),
    };
    BodyCheck {
        name,
        mismatch: false,
        error: Some(outcome),
    }
}

/// Compare one prefix. Read-only: lists metadata and downloads both bodies
/// for every object present on both sides; it never writes or repairs.
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
    let body_checks: Vec<BodyCheck> = futures::stream::iter(
        source_names
            .iter()
            .filter(|name| landed.contains_key(*name))
            .cloned(),
    )
    .map(|name| compare_body(Arc::clone(source), Arc::clone(destination), name))
    .buffered(copy::DEFAULT_CONCURRENCY)
    .collect()
    .await;
    for check in body_checks {
        if check.mismatch {
            diff.body_mismatches.push(check.name);
        } else if let Some(error) = check.error {
            diff.body_errors.push((check.name, error));
        }
    }

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

/// Full post-copy verification. Reads names, metadata, and body bytes from
/// both stores and writes to neither; exits non-zero on any divergence.
async fn verify(args: &StorageVerifyArgs) -> Result<(), CmdError> {
    verify_between(
        args.ends.source(),
        args.ends.destination(),
        &args.prefix,
        args.json,
    )
    .await
}

pub(crate) async fn verify_between(
    from: Endpoint,
    to: Endpoint,
    requested_prefixes: &[String],
    as_json: bool,
) -> Result<(), CmdError> {
    if from.describe() == to.describe() {
        return Err(CmdError::click(format!(
            "source and destination are the same store ({}); there is nothing to compare",
            from.describe()
        )));
    }
    let source = from.build().await?;
    let destination = to.build().await?;
    let prefixes = selected_prefixes(requested_prefixes);

    let diffs: Vec<PrefixDiff> = futures::stream::iter(prefixes.iter())
        .map(|prefix| diff_prefix(&source, &destination, prefix))
        .buffered(copy::DEFAULT_CONCURRENCY)
        .collect()
        .await;

    let missing: usize = diffs.iter().map(|diff| diff.missing.len()).sum();
    let extra: usize = diffs.iter().map(|diff| diff.extra.len()).sum();
    let gaps: usize = diffs.iter().map(|diff| diff.metadata_gaps.len()).sum();
    let body_mismatches: usize = diffs
        .iter()
        .map(|diff| diff.body_mismatches.len())
        .sum();
    let body_errors: usize = diffs.iter().map(|diff| diff.body_errors.len()).sum();
    let diverging = diffs.iter().filter(|diff| diff.diverged()).count();
    let divergent = diffs.iter().any(PrefixDiff::diverged);

    if as_json {
        echo_json(&json!({
            "from": from.describe(),
            "to": to.describe(),
            "prefixes": diffs.iter().map(diff_json).collect::<Vec<Value>>(),
            "missing_at_destination": missing,
            "only_at_destination": extra,
            "metadata_mismatches": gaps,
            "body_mismatches": body_mismatches,
            "body_read_errors": body_errors,
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
         land, {body_mismatches} with different content, {body_errors} with unreadable \
         content. Nothing was copied — re-run `stado storage copy` with the same locators, \
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
        "body_mismatches": diff.body_mismatches,
        "body_read_errors": diff
            .body_errors
            .iter()
            .map(|(name, error)| json!({"name": name, "error": error}))
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
                (diff.body_mismatches.len() + diff.body_errors.len()).to_string(),
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
            "BODY-DIFF",
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
        for name in &diff.body_mismatches {
            println!("  body differs: {name}");
        }
        for (name, error) in &diff.body_errors {
            println!("  body unreadable for {name}: {error}");
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
