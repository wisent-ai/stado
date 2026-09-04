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
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use clap::{Args, Subcommand};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::object_store::OBJECT_API_CHUNK_BYTES;
use crate::queue::copy::{
    self, CopyOptions, CopyPlan, CopyReport, Endpoint, Outcome, CANONICAL_PREFIXES,
};
use crate::queue::{BlobBackend, BlobInfo, JobStorage, StorageError};

use super::table::print as print_table;
use super::CmdError;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageStatReceipt {
    schema: String,
    backend: String,
    bucket: String,
    path: String,
    state: String,
    size: Option<usize>,
    updated_at: Value,
    version: Option<String>,
    metadata: BTreeMap<String, String>,
    detail: Option<String>,
    metadata_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoragePutReceipt {
    schema: String,
    state: String,
    created: bool,
    uri: String,
    sha256: String,
    bytes: usize,
    content_type: String,
}

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
    /// Package one directory as a deterministic gzip-compressed release archive.
    Archive(StorageArchiveArgs),
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

fn parse_storage_kind(raw: &str) -> Result<String, String> {
    crate::capabilities::configurable_variant(crate::capabilities::RuntimeFacet::Storage, raw)
        .map(|variant| variant.id.to_string())
        .ok_or_else(|| {
            let choices =
                crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Storage)
                    .collect::<Vec<_>>()
                    .join(", ");
            format!("unknown storage backend {raw:?}; use one of: {choices}")
        })
}

/// The locator flags shared by `copy` and `verify`, so both commands
/// address a pair of stores with an identical flag set.
#[derive(Args, Debug)]
pub struct EndpointArgs {
    /// Source backend.
    #[arg(long, value_parser = parse_storage_kind)]
    pub(crate) from: String,
    /// Destination backend.
    #[arg(long, value_parser = parse_storage_kind)]
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
pub struct StorageArchiveArgs {
    /// Directory whose contents become the archive root.
    source: String,
    /// New .tar.gz output path. Refuses to overwrite.
    output: String,
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
        StorageCommands::Archive(args) => archive(&args),
        StorageCommands::Put(args) => put(&args).await,
        StorageCommands::Get(args) => get(&args).await,
        StorageCommands::Objects(args) => objects(&args).await,
        StorageCommands::Rm(args) => rm(&args).await,
        StorageCommands::Url(args) => object_url(&args),
    }
}

const ARCHIVE_MAX_ENTRIES: usize = 1_000_000;
const ARCHIVE_MAX_PATH_BYTES: usize = 4 * 1024;
const ARCHIVE_MAX_MEMBER_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const ARCHIVE_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;

fn sorted_archive_paths(
    root: &std::path::Path,
    directory: &std::path::Path,
    paths: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| {
        left.strip_prefix(root)
            .unwrap_or(left)
            .cmp(right.strip_prefix(root).unwrap_or(right))
    });
    for path in entries {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "archive source contains unsupported symlink: {}",
                    path.display()
                ),
            ));
        }
        let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
        if paths.len() >= ARCHIVE_MAX_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "archive source exceeds the one-million-entry limit",
            ));
        }
        if relative.as_os_str().as_encoded_bytes().len() > ARCHIVE_MAX_PATH_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("archive member path exceeds 4096 bytes: {}", path.display()),
            ));
        }
        paths.push(path.clone());
        if metadata.is_dir() {
            sorted_archive_paths(root, &path, paths)?;
        }
    }
    Ok(())
}

fn archive(args: &StorageArchiveArgs) -> Result<(), CmdError> {
    let source = std::path::Path::new(&args.source);
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "archive source must be a real directory: {}",
            source.display()
        )));
    }
    let source = source.canonicalize()?;
    let output = std::path::Path::new(&args.output);
    if output.try_exists()? {
        return Err(CmdError::click(format!(
            "refusing to overwrite archive {}",
            output.display()
        )));
    }
    let output_parent = output
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .canonicalize()?;
    if output_parent.starts_with(&source) {
        return Err(CmdError::click(
            "archive output must be outside the source directory",
        ));
    }
    let mut output_created = false;
    let create_result = (|| -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        output_created = true;
        let encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        archive.mode(tar::HeaderMode::Deterministic);
        archive.follow_symlinks(false);
        let mut paths = Vec::new();
        sorted_archive_paths(&source, &source, &mut paths)?;
        paths.sort_by(|left, right| {
            left.strip_prefix(&source)
                .unwrap_or(left)
                .cmp(right.strip_prefix(&source).unwrap_or(right))
        });
        let mut total_member_bytes = 0_u64;
        for path in paths {
            let name = path.strip_prefix(&source).map_err(std::io::Error::other)?;
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "archive source contains unsupported symlink: {}",
                        path.display()
                    ),
                ));
            }
            let mut open = std::fs::OpenOptions::new();
            open.read(true);
            #[cfg(target_os = "macos")]
            open.custom_flags(0x0000_0100);
            #[cfg(target_os = "linux")]
            open.custom_flags(0x0002_0000);
            let mut member = open.open(&path)?;
            let opened_metadata = member.metadata()?;
            if opened_metadata.is_dir() && metadata.is_dir() {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                header.set_uid(0);
                header.set_gid(0);
                header.set_mtime(0);
                header.set_cksum();
                archive.append_data(&mut header, name, std::io::empty())?;
            } else if opened_metadata.is_file() && metadata.is_file() {
                let member_bytes = opened_metadata.len();
                if member_bytes > ARCHIVE_MAX_MEMBER_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("archive member exceeds the 8 GiB limit: {}", path.display()),
                    ));
                }
                total_member_bytes =
                    total_member_bytes
                        .checked_add(member_bytes)
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "archive member byte total overflowed",
                            )
                        })?;
                if total_member_bytes > ARCHIVE_MAX_TOTAL_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "archive source exceeds the 32 GiB uncompressed limit",
                    ));
                }
                archive.append_file(name, &mut member)?;
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "archive member changed type or is unsupported: {}",
                        path.display()
                    ),
                ));
            }
        }
        let encoder = archive.into_inner()?;
        let file = encoder.finish()?;
        file.sync_all()?;
        drop(file);
        std::fs::File::open(&output_parent)?.sync_all()
    })();
    if let Err(error) = create_result {
        if output_created {
            let _ = std::fs::remove_file(output);
        }
        return Err(CmdError::click(format!(
            "cannot create release archive {}: {error}",
            output.display()
        )));
    }
    let mut file = std::fs::File::open(output)?;
    let mut hasher = Sha256::new();
    let mut buffer = [u8::MIN; u16::MAX as usize];
    loop {
        let read = file.read(&mut buffer)?;
        if read == usize::default() {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let bytes = file.metadata()?.len();
    let sha256 = hex::encode(hasher.finalize());
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "source": source,
                "output": output,
                "bytes": bytes,
                "sha256": sha256,
            }))?
        );
    } else {
        println!("{} bytes sha256={} {}", bytes, sha256, output.display());
    }
    Ok(())
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
    // A copy moves bytes; it must never move the address they live at. The
    // object API is addressed by bare ecosystem keys and every bucket or
    // directory by namespace-qualified store paths, so crossing the two
    // rewrites every name in the set. That is not a hypothetical: it put
    // 9.6 GiB at `ecosystem/probierz/ecosystem/probierz/` in the store the
    // object API serves on charless-mac-mini, and bare `artifacts/`,
    // `status/` and `runs/` trees in that host's backup beside their
    // correctly-qualified twins. Both copies reported success.
    if from.keys_are_namespace_qualified() != to.keys_are_namespace_qualified() {
        let (qualified, bare) = if from.keys_are_namespace_qualified() {
            (from.describe(), to.describe())
        } else {
            (to.describe(), from.describe())
        };
        return Err(CmdError::click(format!(
            "{qualified} names objects by their namespace-qualified store path and {bare} names \
             them by bare ecosystem key, so copying between the two would re-address every \
             object: keys gain a second `ecosystem/<namespace>/` in one direction and lose the \
             one they have in the other. Copy to a store of the same kind, or address the same \
             store through one endpoint on both sides."
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
    let mut blobs = backend
        .list_blobs_with_meta(&backend_prefix(backend, prefix)?)
        .await?;
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
///
/// Five states, not three. Three collapsed every way of not getting an answer
/// into `unreachable`, so a `401` refusal, a `503` boundary that is down and
/// the resolver's own `502 upstream unavailable` all arrived as one verdict,
/// separable only by reading a detail string -- and a caller asking "is this
/// coordinate spent" cannot branch on prose. Two releases turned on that
/// question on 2026-09-03 and got `unreachable` for three different causes
/// with three different remedies. Each of these is something a reader can act
/// on: fix a credential for the refused, retry the unavailable, chase the
/// transport for the unreachable.
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
    /// The store answered and refused the question: this reader may not ask
    /// it. Nothing is known about the object, and asking again unchanged
    /// cannot learn anything.
    Refused(String),
    /// The store answered that it cannot answer right now. Nothing is known
    /// about the object, and the same question may be answered later.
    Unavailable(String),
    /// Nothing answered at all. This is the state `BlobBackend::exists`
    /// cannot express.
    Unreachable(String),
}

impl Presence {
    /// The one-word verdict a script branches on. The exit code says only
    /// whether the question was answered, so the three unanswered states have
    /// to be distinguishable here or they are not distinguishable at all.
    fn state(&self) -> &'static str {
        match self {
            Self::Present { .. } => "present",
            Self::Absent => "absent",
            Self::Refused(_) => "refused",
            Self::Unavailable(_) => "unavailable",
            Self::Unreachable(_) => "unreachable",
        }
    }

    /// Whether the store answered the question that was asked.
    ///
    /// `present` and `absent` are answers; the other three are not. The
    /// exit-code contract turns on exactly this, so a caller can never read a
    /// store that did not answer as a store that answered "gone".
    fn answered(&self) -> bool {
        matches!(self, Self::Present { .. } | Self::Absent)
    }

    fn detail(&self) -> Option<String> {
        match self {
            Self::Present { detail, .. } => detail.clone(),
            Self::Absent => None,
            Self::Refused(detail) | Self::Unavailable(detail) | Self::Unreachable(detail) => {
                Some(detail.clone())
            }
        }
    }

    /// Why this unanswered question stays unanswered, naming what the caller
    /// can do about THIS state rather than about not-answering in general.
    ///
    /// Empty for an answered question, which needs no such sentence. Every
    /// caller reaches this only behind [`Presence::answered`], so that one
    /// predicate stays the single statement of the exit-code contract and
    /// this stays the single statement of the reason.
    fn unanswered_sentence(&self, path: &str) -> String {
        let (verdict, remedy, detail) = match self {
            Self::Present { .. } | Self::Absent => return String::new(),
            Self::Refused(detail) => (
                "REFUSED",
                "the store answered that this reader may not ask: repair the credential or the \
                 grant, because the same question asked again cannot learn anything",
                detail,
            ),
            Self::Unavailable(detail) => (
                "UNAVAILABLE",
                "the store answered that it cannot answer right now: this same question may be \
                 answered later, so retry it",
                detail,
            ),
            Self::Unreachable(detail) => (
                "UNREACHABLE",
                "nothing answered at all: chase the transport in front of the store",
                detail,
            ),
        };
        format!(
            "{path:?} is {verdict}, not absent — {remedy}: {detail}. Treat the object's \
             existence as unknown."
        )
    }
}

/// Which unanswered state one HTTP status is.
///
/// Only ever reached for a status that is not a success, not a redirect and
/// not a 404, so every arm here is a way of not answering the question.
/// `401`/`403` is the store refusing it. `429`/`503` is the store saying "not
/// now" -- the object plane answers `503 object authorization unavailable`
/// when its Skarbiec boundary is down, which is a wait, not a dead store. A
/// gateway status is the proxy in front of the store rather than the store:
/// Stado's own service resolver writes exactly `502 upstream unavailable`
/// when its SSH forward cannot carry a connection, and nothing answered in
/// that case, so it has to stay `unreachable`.
fn unanswered_for_status(status: u16, detail: String) -> Presence {
    match status {
        401 | 403 => Presence::Refused(detail),
        429 | 503 => Presence::Unavailable(detail),
        _ => Presence::Unreachable(detail),
    }
}

/// The same judgement for a backend error, which carries a status when the
/// backend spoke HTTP and carries none when the failure was below HTTP.
fn unanswered_for_error(error: &StorageError) -> Presence {
    let detail = error.to_string();
    match error {
        StorageError::Stado { status, .. } | StorageError::Gcs { status, .. } => {
            unanswered_for_status(*status, detail)
        }
        // Authentication that could not be established is this reader lacking
        // standing to ask, which is a refusal however far below HTTP it
        // happened.
        StorageError::Auth(_) => Presence::Refused(detail),
        _ => Presence::Unreachable(detail),
    }
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
            Err(_) => unanswered_for_error(&err),
        },
    }
}

/// Resolve a CLI path argument to the key the backend actually stores under.
///
/// Two addressing forms reach the commands that take a path: a `stado://` product
/// URI, and a bare queue path, which is already a backend key. Only the explicit
/// scheme is rewritten, so queue callers are untouched.
///
/// WHICH key a URI becomes is the backend's answer, not this module's. The first
/// repair here rewrote every URI into the qualified store path
/// `ecosystem/<namespace>/<key>`, which is right for a filesystem or a bucket and
/// wrong for the object API: that backend re-prefixes with its own namespace, so
/// the qualified path asks it for `ecosystem/<ns>/ecosystem/<ns>/<key>`. With the
/// fleet store bound to the object API, `stat` therefore reported `absent` for
/// objects the same store served over HTTP 200 — the same doubled address a writer
/// defect had already created 417 real objects at.
fn backend_key(backend: &Arc<dyn BlobBackend>, path: &str) -> Result<String, CmdError> {
    if path.starts_with("stado://") {
        Ok(backend.blob_path(&crate::object_store::ObjectRef::parse(path)?))
    } else {
        Ok(path.to_string())
    }
}

/// The same resolution for a listing prefix, which may name a whole namespace and
/// therefore carry no key at all.
fn backend_prefix(backend: &Arc<dyn BlobBackend>, prefix: &str) -> Result<String, CmdError> {
    match prefix.strip_prefix("stado://") {
        Some(rest) => {
            let (namespace, key) = rest.split_once('/').unwrap_or((rest, ""));
            if RemoteObjectApi::release_authorized(namespace, key) {
                return Err(CmdError::click(
                    "release-governed stado:// prefixes must be listed with `stado storage objects \
                     <namespace> <prefix>` so the exact publisher credential is used",
                ));
            }
            Ok(backend.blob_prefix(namespace, key)?)
        }
        None => Ok(prefix.to_string()),
    }
}

/// Exit code contract: zero means the question was ANSWERED (`present` or
/// `absent`), non-zero means it was not (`refused`, `unavailable`,
/// `unreachable`). Scripting an "is it gone?" check on the exit status
/// therefore never mistakes a store that could not answer for a drained one.
///
/// Branch on `state` for which of the five it was. The three non-zero states
/// used to be one word, so a caller that wanted to retry a transient outage,
/// or to stop and fix a credential, had to grep a prose detail line to tell
/// which it was looking at.
async fn stat(args: &StorageStatArgs) -> Result<(), CmdError> {
    // Which store is even being asked. A `stado://releases/...` object lives in the
    // release channel, reached by its own route; the job store never held those bytes,
    // so asking it reports `absent` for every release ever published -- an answer
    // indistinguishable from a real absence. Only the witness differs here: one
    // rendering below reports whichever answered, so the two cannot drift.
    // ONLY a `stado://` argument names a namespace. `ObjectRef::parse` accepts
    // a bare `<namespace>/<key>` too, so a queue path was read as a coordinate
    // in a foreign namespace — `artifacts/models/...` became namespace
    // `artifacts` — and the probe went to the object API's list route, which
    // answered 401 for a namespace this token has no grant on. An object the
    // very same store serves cannot be reported unreachable because the path
    // was spelled without a scheme.
    let parsed = if args.path.starts_with("stado://") {
        crate::object_store::ObjectRef::parse(&args.path)
    } else {
        Err(crate::queue::StorageError::Other(
            "a bare path addresses the queue store, not a namespace".to_string(),
        ))
    };
    let release = match &parsed {
        Ok(object) if object.namespace() == "releases" => {
            RemoteObjectApi::configured_release_reader()?.map(|remote| (remote, object.to_string()))
        }
        _ => None,
    };

    // An object outside the queue namespace is not in the queue store and never
    // can be: `StadoObjectBackend` builds `ObjectRef::new(&self.namespace, path)`,
    // so every probe is re-prefixed with the queue namespace. Asking it about
    // `stado://sources/...` reported `absent` for objects that exist, and I
    // believed that answer twice tonight -- once far enough to publish a source
    // snapshot and repoint a recipe around a file that was never missing.
    let object_api = match (&parsed, &release) {
        (Ok(object), None)
            if RemoteObjectApi::release_authorized(object.namespace(), object.key())
                || object.namespace() != crate::config::wc_stado_storage_namespace() =>
        {
            RemoteObjectApi::configured_for_object(object)?.map(|remote| (remote, object.clone()))
        }
        _ => None,
    };

    let (presence, store_bucket, store_backend, metadata, updated, metadata_error) = match release {
        Some((remote, uri)) => {
            let presence = remote.stat_release(&uri).await?;
            // The channel answers presence, not bookkeeping: it serves bytes by
            // redirect, so there is no listing to carry metadata or a timestamp.
            // Reporting empty is honest; inventing them from the job store would
            // attach one store's bookkeeping to another store's answer.
            (
                presence,
                remote.base_url.to_string(),
                "release-channel".to_string(),
                BTreeMap::new(),
                None,
                None,
            )
        }
        None if object_api.is_some() => {
            let (remote, object) = object_api.expect("checked by the guard above");
            // The list route is the one surface proven to answer for these
            // namespaces, and it carries the bookkeeping the queue listing would
            // have supplied, so nothing is reported as empty that is known.
            let entries = remote.list(object.namespace(), object.key()).await?;
            let uri = object.to_string();
            let entry = entries
                .into_iter()
                .find(|value| value.get("uri").and_then(Value::as_str) == Some(uri.as_str()));
            let (presence, metadata, updated) = match entry {
                Some(value) => (
                    Presence::Present {
                        size: usize::try_from(
                            value
                                .get("size")
                                .or_else(|| value.get("bytes"))
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                        )
                        .unwrap_or(usize::MAX),
                        version: None,
                        detail: Some(
                            "the list route reports size and metadata; it carries no CAS version"
                                .to_string(),
                        ),
                    },
                    value
                        .get("metadata")
                        .and_then(Value::as_object)
                        .map(|fields| {
                            fields
                                .iter()
                                .filter_map(|(name, item)| {
                                    item.as_str().map(|text| (name.clone(), text.to_string()))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    value
                        .get("updated_at")
                        .and_then(Value::as_str)
                        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                        .map(|stamp| stamp.with_timezone(&Utc)),
                ),
                None => (Presence::Absent, BTreeMap::new(), None),
            };
            (
                presence,
                remote.base_url.to_string(),
                "object-api".to_string(),
                metadata,
                updated,
                None,
            )
        }
        None => {
            let store = JobStorage::new().await?;
            let backend = store.backend();
            let probe_path = backend_key(backend, &args.path)?;
            let presence = probe(backend, &probe_path).await;

            // Metadata and the timestamp come from the listing, which is a separate
            // grant from object read: if listing is denied while the read worked,
            // say so rather than downgrading a known-present object to unreachable.
            let (metadata, updated, metadata_error) = match &presence {
                Presence::Present { .. } => match backend.list_blobs_with_meta(&probe_path).await {
                    Ok(blobs) => blobs
                        .into_iter()
                        .find(|blob| blob.name == probe_path)
                        .map_or_else(
                            || (BTreeMap::new(), None, None),
                            |blob| (blob.metadata, blob.updated, None),
                        ),
                    Err(err) => (BTreeMap::new(), None, Some(err.to_string())),
                },
                // Nothing else is known to be there, so there is no listing
                // worth asking for and no metadata to carry.
                _ => (BTreeMap::new(), None, None),
            };
            (
                presence,
                store.bucket_name().to_string(),
                store.backend_name().to_string(),
                metadata,
                updated,
                metadata_error,
            )
        }
    };

    let (state, size, version, detail) = (
        presence.state(),
        match &presence {
            Presence::Present { size, .. } => Some(*size),
            _ => None,
        },
        match &presence {
            Presence::Present { version, .. } => version.clone(),
            _ => None,
        },
        presence.detail(),
    );

    if args.json {
        echo_json(&serde_json::to_value(StorageStatReceipt {
            schema: "stado.storage-stat-receipt.v1".into(),
            backend: store_backend,
            bucket: store_bucket,
            path: args.path.clone(),
            state: state.into(),
            size,
            updated_at: render_optional_stamp(updated),
            version,
            metadata,
            detail,
            metadata_error,
        })?)?;
    } else {
        let mut rows = vec![
            vec!["path".to_string(), args.path.clone()],
            vec!["state".to_string(), state.to_string()],
            vec![
                "store".to_string(),
                format!("{store_bucket} ({store_backend})"),
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
                "\nThe store ANSWERED: {:?} is not there. This is not the same as a store \
                 that refused the question, could not answer it now, or could not be \
                 reached at all.",
                args.path
            );
        }
    }

    // The exit-code contract: zero means the question was ANSWERED (`present`
    // or `absent`), non-zero means it was not (`refused`, `unavailable`,
    // `unreachable`). Scripting an "is it gone?" check on the exit status
    // therefore never mistakes a store that could not answer for a drained
    // one; branch on `state` for which answer, and for which kind of silence.
    if presence.answered() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{}{}",
        presence.unanswered_sentence(&args.path),
        inferred_namespace_hint(&args.path)
    )))
}

/// The sentence a bare path earns when the store refuses it.
///
/// `ObjectRef::parse` accepts `<namespace>/<key>` as well as
/// `stado://<namespace>/<key>`, so a path without the scheme still names a
/// namespace — the FIRST SEGMENT — and the probe goes wherever that points.
/// For release objects this is a trap with no tell, because a release key
/// itself begins with the product name: `stado/0.13.20/darwin-arm64/SHA256SUMS`
/// reads like a complete coordinate and is actually namespace `stado`, the
/// queue store, where the caller has no grant. The object API then answers
/// `HTTP 401 {"error":"unauthorized"}` — a true statement about a namespace
/// nobody meant to ask about, which `CmdError` classifies as `auth` and
/// reports as "the credentials this command used were rejected".
///
/// On 2026-09-01 that cost an investigation into a credential that was never
/// broken: the same object answered `present` through
/// `stado://releases/stado/0.13.20/darwin-arm64/SHA256SUMS` in the same second,
/// and `/api/release/object` served it unauthenticated. The refusal was
/// correct; only its attribution was wrong.
///
/// So the hint is attached to the refusal rather than the state: the exit-code
/// contract still reports `unreachable`, because the store this path named
/// genuinely did not answer for it.
fn inferred_namespace_hint(path: &str) -> String {
    if path.starts_with("stado://") {
        return String::new();
    }
    let Some((namespace, key)) = path.split_once('/') else {
        return String::new();
    };
    if namespace.is_empty() || key.is_empty() {
        return String::new();
    }
    format!(
        " This path has no `stado://` scheme, so its first segment was read as \
         the namespace: it asked the {namespace:?} store about {key:?}, which is \
         probably not what you meant. Name the namespace explicitly — \
         `stado://<namespace>/<key>` — and note that published release objects \
         live under `stado://releases/`, whose keys start with the product \
         name: `stado://releases/{path}`."
    )
}

// ---- cat ----

/// Stream one object's body to stdout. Job documents, `registry.json` and
/// the health beacons are all JSON blobs an operator reads directly.
///
/// `read_bytes` propagates [`crate::queue::StorageError`], so an
/// unreachable store is an error here too and never an empty body.
async fn cat(args: &StorageCatArgs) -> Result<(), CmdError> {
    let bytes = if args.path.starts_with("stado://") {
        fetch_object(&args.path).await?
    } else {
        let store = JobStorage::new().await?;
        let Some(bytes) = store.read_bytes(&args.path).await? else {
            return Err(CmdError::click(format!(
                "{:?}: absent — the store answered and the object is not there",
                args.path
            )));
        };
        bytes
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

enum RemoteObjectAuth {
    Generic(String),
    PublisherOnly,
    Public,
}

struct RemoteObjectApi {
    http: reqwest::Client,
    base_url: url::Url,
    auth: RemoteObjectAuth,
}

#[derive(serde::Deserialize)]
struct RemotePutResponse {
    state: String,
    uri: String,
    content_type: String,
}

#[derive(serde::Serialize)]
struct RemoteComposeChunk {
    uri: String,
    size: usize,
    sha256: String,
}

#[derive(serde::Serialize)]
struct RemoteComposeRequest<'a> {
    uri: &'a str,
    content_type: &'a str,
    if_absent: bool,
    metadata: &'a BTreeMap<String, String>,
    upload_id: &'a str,
    size: usize,
    chunks: &'a [RemoteComposeChunk],
}

#[derive(serde::Deserialize)]
struct RemoteComposeResponse {
    status: u16,
    payload: Value,
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

fn partial_content_bounds(
    response: &reqwest::Response,
    expected_start: usize,
    operation: &str,
) -> Result<(usize, usize), CmdError> {
    let content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            CmdError::click(format!(
                "Stado object API {operation} partial response carries no Content-Range"
            ))
        })?;
    let (range, total) = content_range
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .ok_or_else(|| {
            CmdError::click(format!(
                "Stado object API {operation} returned invalid Content-Range {content_range:?}"
            ))
        })?;
    let (start, end) = range.split_once('-').ok_or_else(|| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let start = start.parse::<usize>().map_err(|_| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let end = end.parse::<usize>().map_err(|_| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let total = total.parse::<usize>().map_err(|_| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let end_exclusive = end.checked_add(1).ok_or_else(|| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    if start != expected_start
        || end < start
        || end_exclusive > total
        || end_exclusive.saturating_sub(start) > OBJECT_API_CHUNK_BYTES
    {
        return Err(CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?} \
             for byte offset {expected_start}"
        )));
    }
    Ok((end_exclusive, total))
}

impl RemoteObjectApi {
    /// The configured object-API endpoint, or `None` when this process reads a
    /// disk-backed store directly.
    ///
    /// `STADO_API_URL` wins, but a deployment whose queue backend already IS
    /// the object API needs no second declaration: without this fallback,
    /// `fetch_object` handed an already-namespaced `storage_path()` to a
    /// backend that namespaces again, so every read resolved to
    /// `ecosystem/<ns>/ecosystem/<ns>/...` and answered "absent".
    fn endpoint_from_env_or_config() -> Result<Option<url::Url>, CmdError> {
        if let Some(url) = configured_object_base_url("STADO_API_URL")? {
            return Ok(Some(url));
        }
        if crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
            != Some(crate::capabilities::StorageAdapter::StadoObject)
        {
            return Ok(None);
        }
        let configured = crate::config::wc_stado_storage_url();
        if configured.trim().is_empty() {
            return Ok(None);
        }
        url::Url::parse(configured.trim())
            .map(Some)
            .map_err(|error| CmdError::click(format!("storage.stado.url is not a URL: {error}")))
    }

    fn configured() -> Result<Option<Self>, CmdError> {
        let Some(base_url) = Self::endpoint_from_env_or_config()? else {
            return Ok(None);
        };
        let token = match std::env::var("STADO_API_TOKEN") {
            Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
            Ok(_) | Err(std::env::VarError::NotPresent) => {
                let token_file = std::env::var("STADO_API_TOKEN_FILE")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| crate::config::wc_stado_storage_token_file().to_string());
                if token_file.trim().is_empty() {
                    return Err(CmdError::click(
                        "STADO_API_TOKEN, STADO_API_TOKEN_FILE or storage.stado.token_file \
                         is required to reach the object API",
                    ));
                }
                let path = crate::config_file::expand_tilde(token_file.trim());
                let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                    CmdError::click(format!(
                        "cannot inspect STADO_API_TOKEN_FILE {}: {error}",
                        path.display()
                    ))
                })?;
                if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                    return Err(CmdError::click(format!(
                        "STADO_API_TOKEN_FILE must be a regular file: {}",
                        path.display()
                    )));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o077 != 0 {
                        return Err(CmdError::click(format!(
                            "STADO_API_TOKEN_FILE must be owner-only (chmod 600): {}",
                            path.display()
                        )));
                    }
                }
                let value = std::fs::read_to_string(&path).map_err(|error| {
                    CmdError::click(format!(
                        "cannot read STADO_API_TOKEN_FILE {}: {error}",
                        path.display()
                    ))
                })?;
                let token = value.trim();
                if token.is_empty()
                    || token
                        .chars()
                        .any(|character| matches!(character, '\r' | '\n'))
                {
                    return Err(CmdError::click(
                        "STADO_API_TOKEN_FILE is empty or malformed",
                    ));
                }
                token.to_string()
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(CmdError::click(
                    "STADO_API_TOKEN must be valid Unicode when STADO_API_URL is configured",
                ));
            }
        };
        let http = Self::http_client()?;
        Ok(Some(Self {
            http,
            base_url,
            auth: RemoteObjectAuth::Generic(token),
        }))
    }

    fn configured_with_auth(auth: RemoteObjectAuth) -> Result<Option<Self>, CmdError> {
        let Some(base_url) = Self::endpoint_from_env_or_config()? else {
            return Ok(None);
        };
        Ok(Some(Self {
            http: Self::http_client()?,
            base_url,
            auth,
        }))
    }

    fn configured_release_reader() -> Result<Option<Self>, CmdError> {
        Self::configured_with_auth(RemoteObjectAuth::Public)
    }

    /// Release writes and exact release listings resolve their publisher bearer
    /// from Skarbiec. Constructing that path must not first demand the generic
    /// object credential it deliberately does not present.
    fn configured_release_writer() -> Result<Option<Self>, CmdError> {
        Self::configured_with_auth(RemoteObjectAuth::PublisherOnly)
    }

    fn release_authorized(namespace: &str, key_or_prefix: &str) -> bool {
        matches!(namespace, "releases" | "sources")
            || (namespace == "system" && key_or_prefix.starts_with("release-catalog/"))
    }

    fn configured_for_object(
        object: &crate::object_store::ObjectRef,
    ) -> Result<Option<Self>, CmdError> {
        if Self::release_authorized(object.namespace(), object.key()) {
            Self::configured_release_writer()
        } else {
            Self::configured()
        }
    }

    fn configured_for_list(namespace: &str, prefix: &str) -> Result<Option<Self>, CmdError> {
        if Self::release_authorized(namespace, prefix) {
            Self::configured_release_writer()
        } else {
            Self::configured()
        }
    }

    fn http_client() -> Result<reqwest::Client, CmdError> {
        fleet_https_client()
    }

    fn endpoint(&self, route: &str, query: &[(&str, &str)]) -> Result<url::Url, CmdError> {
        object_api_endpoint(&self.base_url, route, query)
    }

    fn request(&self, method: reqwest::Method, endpoint: url::Url) -> reqwest::RequestBuilder {
        self.request_as(method, endpoint, None)
    }

    /// Sign according to the constructor-selected authentication mode.
    /// Generic clients always use their configured object credential,
    /// publisher clients use only the explicitly resolved publisher bearer,
    /// and public clients never attach authorization.
    fn request_as(
        &self,
        method: reqwest::Method,
        endpoint: url::Url,
        publisher_bearer: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let request = self.http.request(method, endpoint);
        let bearer = match &self.auth {
            RemoteObjectAuth::Generic(token) => Some(token.as_str()),
            RemoteObjectAuth::PublisherOnly => publisher_bearer,
            RemoteObjectAuth::Public => None,
        };
        match bearer {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// The credential a write to `uri` must present.
    ///
    /// Resolved from `release_api.publishers` -- the same table the server
    /// compares against in `authorize_release` -- so both ends of one
    /// authorization check read one declaration and cannot disagree. The
    /// coordinator storage token is not a release credential, and presenting it
    /// returned a `401` that named neither the table nor the item it wanted.
    async fn release_bearer(&self, uri: &str) -> Result<Option<String>, CmdError> {
        let object = crate::object_store::ObjectRef::parse(uri)?;
        self.release_bearer_for(object.namespace(), object.key())
            .await
    }

    /// The same resolution for a namespace and key or prefix, which is how the
    /// list route addresses objects.
    async fn release_bearer_for(
        &self,
        namespace: &str,
        key_or_prefix: &str,
    ) -> Result<Option<String>, CmdError> {
        let Some(policy_key) = crate::object_store::release_policy_key(namespace, key_or_prefix)
        else {
            if Self::release_authorized(namespace, key_or_prefix) {
                return Err(CmdError::click(format!(
                    "{namespace}/{key_or_prefix} does not resolve to one declared release publisher"
                )));
            }
            return Ok(None);
        };
        let publisher = crate::config::release_publisher_for_key(&policy_key).ok_or_else(|| {
            CmdError::click(format!(
                "release_api.publishers declares no publisher for {policy_key}"
            ))
        })?;
        // Read with the publisher command's configured consumer, whose grant
        // is settled here. The server has a separate release verifier; using
        // that identity in the client would ignore the grant just acquired.
        // An existing authorized read must still work when this caller lacks
        // the owner credentials required to extend its grant.
        if let Err(error) =
            crate::credential_store::grant::settle_field_reads(publisher.item(), &["token"])
        {
            eprintln!(
                "could not widen the grant on release publisher item {} before reading it, \
                 continuing with the grant as it stands: {error}",
                publisher.item()
            );
        }
        let token = crate::credential_store::read_string(publisher.item(), "token")
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot read release publisher item {}: {error}",
                    publisher.item()
                ))
            })?
            .ok_or_else(|| {
                CmdError::click(format!(
                    "release publisher item {} carries no token field",
                    publisher.item()
                ))
            })?;
        // A token that cannot become a header value is refused here, by name.
        // reqwest reports that case as the bare string `builder error`, with no
        // item, no field and no failure point that means anything: on
        // 2026-09-03 `stado storage stat
        // stado://system/release-catalog/preferences-landing.json` answered
        // exactly that, and the same command for two other products answered
        // an honest HTTP 401, so the operator's only signal that the fault was
        // in a credential and not in the network was that one product differed
        // from the others. A bearer is header material; whether one is usable
        // is knowable before the request, and the answer names the item.
        if token.is_empty() {
            return Err(CmdError::click(format!(
                "release publisher item {} carries an empty token field",
                publisher.item()
            )));
        }
        if reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_err() {
            return Err(CmdError::click(format!(
                "release publisher item {}'s token field cannot form an Authorization header: it \
                 is {} bytes and carries a character a header value may not (a newline or a \
                 control byte, most often a trailing newline stored with the value). Rewrite the \
                 field with the value alone",
                publisher.item(),
                token.len()
            )));
        }
        Ok(Some(token))
    }

    async fn put_chunked(
        &self,
        uri: &str,
        content_type: &str,
        if_absent: bool,
        bytes: bytes::Bytes,
        metadata: &BTreeMap<String, String>,
        bearer: Option<&str>,
    ) -> Result<RemotePutResponse, CmdError> {
        let object = crate::object_store::ObjectRef::parse(uri)?;
        let upload_id = hex::encode(Sha256::digest(&bytes));
        let mut chunks = Vec::with_capacity(bytes.len().div_ceil(OBJECT_API_CHUNK_BYTES));
        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = offset
                .saturating_add(OBJECT_API_CHUNK_BYTES)
                .min(bytes.len());
            let chunk = bytes.slice(offset..end);
            let index = chunks.len();
            let sha256 = hex::encode(Sha256::digest(&chunk));
            let chunk_object = crate::object_store::ObjectRef::new(
                object.namespace(),
                &format!("{}.__stado_upload/{upload_id}/{index:08}", object.key()),
            )?;
            let chunk_uri = chunk_object.to_string();
            let endpoint = self.endpoint(
                "/api/object",
                &[("uri", chunk_uri.as_str()), ("if_absent", "true")],
            )?;
            let chunk_metadata = BTreeMap::from([
                ("stado-upload-id".to_string(), upload_id.clone()),
                ("stado-upload-index".to_string(), index.to_string()),
                ("stado-upload-sha256".to_string(), sha256.clone()),
                ("stado-upload-target".to_string(), uri.to_string()),
            ]);
            let response = self
                .request_as(reqwest::Method::PUT, endpoint, bearer)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .header(
                    "x-stado-object-metadata",
                    serde_json::to_string(&chunk_metadata)?,
                )
                .body(chunk)
                .send()
                .await?;
            if !matches!(
                response.status(),
                reqwest::StatusCode::CONFLICT | reqwest::StatusCode::PRECONDITION_FAILED
            ) {
                let stored: RemotePutResponse = self
                    .response_json(response, "object chunk PUT", bearer)
                    .await?;
                if stored.state != "stored"
                    || stored.uri != chunk_uri
                    || stored.content_type != "application/octet-stream"
                {
                    return Err(CmdError::click(
                        "Stado object API returned an inconsistent object chunk PUT response",
                    ));
                }
            }
            chunks.push(RemoteComposeChunk {
                uri: chunk_uri,
                size: end - offset,
                sha256,
            });
            offset = end;
        }

        let endpoint = self.endpoint("/api/object/compose", &[])?;
        let request = RemoteComposeRequest {
            uri,
            content_type,
            if_absent,
            metadata,
            upload_id: &upload_id,
            size: bytes.len(),
            chunks: &chunks,
        };
        let response = self
            .request_as(reqwest::Method::POST, endpoint, bearer)
            .json(&request)
            .send()
            .await?;
        let response: RemoteComposeResponse = self
            .response_json(response, "object chunk composition", bearer)
            .await?;
        let status = reqwest::StatusCode::from_u16(response.status)
            .map_err(|_| CmdError::click("object composition returned an invalid HTTP status"))?;
        if !status.is_success() {
            let payload = response.payload.to_string();
            let detail = response_body_detail(payload.as_bytes(), self.generic_bearer(), bearer);
            return Err(CmdError::click(format!(
                "Stado object API returned HTTP {status}: {detail}"
            )));
        }
        serde_json::from_value(response.payload).map_err(|error| {
            CmdError::click(format!(
                "Stado object API returned an invalid object composition payload: {error}"
            ))
        })
    }

    async fn put_with_metadata(
        &self,
        uri: &str,
        content_type: &str,
        if_absent: bool,
        bytes: Vec<u8>,
        metadata: &BTreeMap<String, String>,
    ) -> Result<(), CmdError> {
        let create_only = if_absent;
        let if_absent = if if_absent { "true" } else { "false" };
        let endpoint = self.endpoint("/api/object", &[("uri", uri), ("if_absent", if_absent)])?;
        let bearer = self.release_bearer(uri).await?;
        let bytes = bytes::Bytes::from(bytes);
        // The writer cannot answer until the backend has durably stored the body.
        // Sending a large object as one request therefore spends the client's
        // entire inactivity window waiting for response headers, then retries the
        // same doomed transfer from byte zero. Chunk before that request: every
        // piece stays below the progress deadline and composition remains the one
        // atomic publication of the target object.
        if bytes.len() > OBJECT_API_CHUNK_BYTES {
            let payload = self
                .put_chunked(
                    uri,
                    content_type,
                    create_only,
                    bytes,
                    metadata,
                    bearer.as_deref(),
                )
                .await?;
            if payload.state != "stored"
                || payload.uri != uri
                || payload.content_type != content_type
            {
                return Err(CmdError::click(
                    "Stado object API returned an inconsistent object composition response",
                ));
            }
            return Ok(());
        }
        let response = self
            .request_as(reqwest::Method::PUT, endpoint, bearer.as_deref())
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .header("x-stado-object-metadata", serde_json::to_string(metadata)?)
            .body(bytes.clone())
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            let payload = self
                .put_chunked(
                    uri,
                    content_type,
                    create_only,
                    bytes,
                    metadata,
                    bearer.as_deref(),
                )
                .await?;
            if payload.state != "stored"
                || payload.uri != uri
                || payload.content_type != content_type
            {
                return Err(CmdError::click(
                    "Stado object API returned an inconsistent object composition response",
                ));
            }
            return Ok(());
        }
        // Name the object and the credential that was presented. The bare
        // `401 unauthorized or non-immutable release write` named neither, and
        // reading it cost a day: the same sentence covers a missing bearer, a
        // wrong bearer, and a create-only rewrite, which need opposite fixes.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let presented = match crate::object_store::ObjectRef::parse(uri)
                .ok()
                .and_then(|object| {
                    crate::object_store::release_policy_key(object.namespace(), object.key())
                })
                .and_then(|key| crate::config::release_publisher_for_key(&key))
            {
                Some(publisher) => format!("publisher item {}", publisher.item()),
                None if bearer.is_some() => "a resolved release credential".to_string(),
                None => "the coordinator storage token".to_string(),
            };
            let refusal = self.response_error(response, bearer.as_deref()).await;
            return Err(CmdError::click(format!(
                "{refusal}; PUT {uri} with if_absent={if_absent} presented {presented}"
            )));
        }
        let payload: RemotePutResponse = self
            .response_json(response, "object PUT", bearer.as_deref())
            .await?;
        if payload.state != "stored" || payload.uri != uri || payload.content_type != content_type {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object PUT response",
            ));
        }
        Ok(())
    }

    async fn get(&self, uri: &str) -> Result<Vec<u8>, CmdError> {
        self.get_object_resumable(uri, false)
            .await?
            .ok_or_else(|| CmdError::click(format!("object disappeared during GET: {uri}")))
    }

    async fn get_optional(&self, uri: &str) -> Result<Option<Vec<u8>>, CmdError> {
        self.get_object_resumable(uri, true).await
    }

    async fn get_object_in_ranges(
        &self,
        endpoint: url::Url,
        bearer: Option<&str>,
        optional: bool,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut failures = 0usize;

        'download: loop {
            let start = body.len();
            let end = start
                .saturating_add(OBJECT_API_CHUNK_BYTES.saturating_sub(1))
                .min(limit.saturating_sub(1));
            let response = match self
                .request_as(reqwest::Method::GET, endpoint.clone(), bearer)
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    if failures > 3 {
                        return Err(CmdError::click(format!(
                            "authenticated object GET exhausted its byte-range retries after \
                             {start} bytes: {error}"
                        )));
                    }
                    continue;
                }
            };
            if response.status() == reqwest::StatusCode::NOT_FOUND && body.is_empty() && optional {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(self.response_error(response, bearer).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API object GET refused the byte range beginning at {start}"
                )));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "object GET")?;
            if total > limit {
                return Err(CmdError::click(format!(
                    "Stado object API object GET response exceeds the {limit}-byte limit"
                )));
            }
            body.reserve(total.saturating_sub(body.capacity()));

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > end_exclusive.saturating_sub(body.len()) {
                            return Err(CmdError::click(
                                "Stado object API object GET sent bytes outside the requested \
                                 range",
                            ));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if body.len() != end_exclusive => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "authenticated object GET exhausted its byte-range retries after \
                                 {} of {end_exclusive} bytes",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                    Ok(None) if body.len() == total => return Ok(Some(body)),
                    Ok(None) => {
                        failures = 0;
                        continue 'download;
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "authenticated object GET exhausted its byte-range retries after \
                                 {} bytes: {error}",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                }
            }
        }
    }

    async fn get_object_resumable(
        &self,
        uri: &str,
        optional: bool,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri)])?;
        let bearer = self.release_bearer(uri).await?;
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut last_read_error = None;

        for recovery in 0..=3 {
            let mut request =
                self.request_as(reqwest::Method::GET, endpoint.clone(), bearer.as_deref());
            if !body.is_empty() {
                request = request.header(reqwest::header::RANGE, format!("bytes={}-", body.len()));
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    last_read_error = Some(format!(
                        "error sending authenticated object GET after {} bytes: {}",
                        body.len(),
                        super::http_failure(&error)
                    ));
                    if recovery == 3 {
                        break;
                    }
                    continue;
                }
            };
            if response.status() == reqwest::StatusCode::NOT_FOUND && body.is_empty() && optional {
                return Ok(None);
            }
            if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE && body.is_empty() {
                drop(response);
                return self
                    .get_object_in_ranges(endpoint, bearer.as_deref(), optional)
                    .await;
            }
            if !response.status().is_success() {
                return Err(self.response_error(response, bearer.as_deref()).await);
            }
            if !body.is_empty() && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API object GET refused byte resume at offset {}",
                    body.len()
                )));
            }

            let expected_start = body.len();
            let total = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
                let content_range = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        CmdError::click(
                            "Stado object API object GET partial response carries no Content-Range",
                        )
                    })?;
                let (range, total) = content_range
                    .strip_prefix("bytes ")
                    .and_then(|value| value.split_once('/'))
                    .ok_or_else(|| {
                        CmdError::click(format!(
                            "Stado object API object GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                let (start, _) = range.split_once('-').ok_or_else(|| {
                    CmdError::click(format!(
                        "Stado object API object GET returned invalid Content-Range \
                         {content_range:?}"
                    ))
                })?;
                let start = start.parse::<usize>().map_err(|_| {
                    CmdError::click(format!(
                        "Stado object API object GET returned invalid Content-Range \
                         {content_range:?}"
                    ))
                })?;
                let total = total.parse::<usize>().map_err(|_| {
                    CmdError::click(format!(
                        "Stado object API object GET returned invalid Content-Range \
                         {content_range:?}"
                    ))
                })?;
                if start != expected_start {
                    return Err(CmdError::click(format!(
                        "Stado object API object GET resumed at byte {start}, expected \
                         {expected_start}"
                    )));
                }
                Some(total)
            } else {
                response
                    .content_length()
                    .and_then(|length| usize::try_from(length).ok())
            };
            if total.is_some_and(|total| total > limit) {
                return Err(CmdError::click(format!(
                    "Stado object API object GET response exceeds the {limit}-byte limit"
                )));
            }
            if let Some(total) = total {
                body.reserve(total.saturating_sub(body.capacity()));
            }

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > limit.saturating_sub(body.len()) {
                            return Err(CmdError::click(format!(
                                "Stado object API object GET response exceeds the \
                                 {limit}-byte limit"
                            )));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if total.is_some_and(|total| body.len() != total) => {
                        last_read_error = Some(format!(
                            "Stado object API object GET response ended after {} of {} bytes",
                            body.len(),
                            total.unwrap_or_default()
                        ));
                        break;
                    }
                    Ok(None) => return Ok(Some(body)),
                    Err(error) => {
                        last_read_error = Some(format!(
                            "Stado object API object GET response body connection closed before \
                             completion after {} bytes: {error}",
                            body.len()
                        ));
                        break;
                    }
                }
            }
            if recovery == 3 {
                break;
            }
        }

        Err(CmdError::click(last_read_error.unwrap_or_else(|| {
            "authenticated object GET exhausted its byte-resume attempts".to_string()
        })))
    }

    async fn get_versioned(&self, uri: &str) -> Result<Option<(Vec<u8>, String)>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri), ("versioned", "true")])?;
        let bearer = self.release_bearer(uri).await?;
        let response = self
            .request_as(reqwest::Method::GET, endpoint, bearer.as_deref())
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(self.response_error(response, bearer.as_deref()).await);
        }
        let version = response
            .headers()
            .get("x-stado-version")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CmdError::click("Stado object API omitted the CAS version"))?
            .to_string();
        let bytes = self
            .success_body(
                response,
                max_object_api_download_body(),
                "versioned object GET",
                bearer.as_deref(),
            )
            .await?;
        Ok(Some((bytes, version)))
    }

    async fn put_if_version(
        &self,
        uri: &str,
        content_type: &str,
        expected_version: &str,
        bytes: Vec<u8>,
    ) -> Result<(), CmdError> {
        let endpoint = self.endpoint(
            "/api/object",
            &[("uri", uri), ("if_version", expected_version)],
        )?;
        let bearer = self.release_bearer(uri).await?;
        let response = self
            .request_as(reqwest::Method::PUT, endpoint, bearer.as_deref())
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let presented = crate::object_store::ObjectRef::parse(uri)
                .ok()
                .and_then(|object| {
                    crate::object_store::release_policy_key(object.namespace(), object.key())
                })
                .and_then(|key| crate::config::release_publisher_for_key(&key))
                .map(|publisher| format!("publisher item {}", publisher.item()))
                .unwrap_or_else(|| {
                    if bearer.is_some() {
                        "a resolved release credential".to_string()
                    } else {
                        "the coordinator storage token".to_string()
                    }
                });
            let refusal = self.response_error(response, bearer.as_deref()).await;
            return Err(CmdError::click(format!(
                "{refusal}; conditional PUT {uri} with if_version={expected_version:?} presented \
                 {presented}"
            )));
        }
        let payload: Value = self
            .response_json(response, "conditional object PUT", bearer.as_deref())
            .await?;
        if payload.get("state").and_then(Value::as_str) != Some("stored")
            || payload.get("uri").and_then(Value::as_str) != Some(uri)
        {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent conditional PUT response",
            ));
        }
        Ok(())
    }

    async fn get_release_in_ranges(&self, origin: url::Url) -> Result<Vec<u8>, CmdError> {
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut failures = 0usize;

        'download: loop {
            let start = body.len();
            let end = start
                .saturating_add(OBJECT_API_CHUNK_BYTES.saturating_sub(1))
                .min(limit.saturating_sub(1));
            let mut endpoint = origin.clone();
            let mut selected = None;
            for hop in 0..=3 {
                let request = if hop == 0 {
                    self.request(reqwest::Method::GET, endpoint.clone())
                } else {
                    self.http.get(endpoint.clone())
                }
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
                let response = match request.send().await {
                    Ok(response) => response,
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "public release GET exhausted its byte-range retries after \
                                 {start} bytes: {error}"
                            )));
                        }
                        continue 'download;
                    }
                };
                if response.status().is_redirection() {
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| CmdError::click("release redirect carries no Location"))?;
                    endpoint = response.url().join(location).map_err(|error| {
                        CmdError::click(format!("invalid release redirect: {error}"))
                    })?;
                    continue;
                }
                selected = Some(response);
                break;
            }
            let Some(response) = selected else {
                return Err(CmdError::click("too many release download redirects"));
            };
            if !response.status().is_success() {
                return Err(self.response_error(response, None).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API release GET refused the byte range beginning at {start}"
                )));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "release GET")?;
            if total > limit {
                return Err(CmdError::click(format!(
                    "Stado object API release GET response exceeds the {limit}-byte limit"
                )));
            }
            body.reserve(total.saturating_sub(body.capacity()));

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > end_exclusive.saturating_sub(body.len()) {
                            return Err(CmdError::click(
                                "Stado object API release GET sent bytes outside the requested \
                                 range",
                            ));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if body.len() != end_exclusive => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "public release GET exhausted its byte-range retries after {} of \
                                 {end_exclusive} bytes",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                    Ok(None) if body.len() == total => return Ok(body),
                    Ok(None) => {
                        failures = 0;
                        continue 'download;
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "public release GET exhausted its byte-range retries after {} \
                                 bytes: {error}",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                }
            }
        }
    }

    async fn get_release(&self, uri: &str) -> Result<Vec<u8>, CmdError> {
        let origin = self.endpoint("/api/release/object", &[("uri", uri)])?;
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut last_read_error = None;

        for recovery in 0..=3 {
            let mut endpoint = origin.clone();
            for hop in 0..=3 {
                let mut request = if hop == 0 {
                    self.request(reqwest::Method::GET, endpoint.clone())
                } else {
                    self.http.get(endpoint.clone())
                };
                if !body.is_empty() {
                    request =
                        request.header(reqwest::header::RANGE, format!("bytes={}-", body.len()));
                }
                let response = request.send().await?;
                if response.status().is_redirection() {
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| CmdError::click("release redirect carries no Location"))?;
                    endpoint = response.url().join(location).map_err(|error| {
                        CmdError::click(format!("invalid release redirect: {error}"))
                    })?;
                    continue;
                }
                if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE && body.is_empty() {
                    drop(response);
                    return self.get_release_in_ranges(origin).await;
                }
                if !response.status().is_success() {
                    return Err(self.response_error(response, None).await);
                }
                if !body.is_empty() && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                    return Err(CmdError::click(format!(
                        "Stado object API release GET refused byte resume at offset {}",
                        body.len()
                    )));
                }

                let expected_start = body.len();
                let total = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
                    let content_range = response
                        .headers()
                        .get(reqwest::header::CONTENT_RANGE)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            CmdError::click(
                                "Stado object API release GET partial response carries no \
                                 Content-Range",
                            )
                        })?;
                    let (range, total) = content_range
                        .strip_prefix("bytes ")
                        .and_then(|value| value.split_once('/'))
                        .ok_or_else(|| {
                            CmdError::click(format!(
                                "Stado object API release GET returned invalid Content-Range \
                                 {content_range:?}"
                            ))
                        })?;
                    let (start, _) = range.split_once('-').ok_or_else(|| {
                        CmdError::click(format!(
                            "Stado object API release GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                    let start = start.parse::<usize>().map_err(|_| {
                        CmdError::click(format!(
                            "Stado object API release GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                    let total = total.parse::<usize>().map_err(|_| {
                        CmdError::click(format!(
                            "Stado object API release GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                    if start != expected_start {
                        return Err(CmdError::click(format!(
                            "Stado object API release GET resumed at byte {start}, expected \
                             {expected_start}"
                        )));
                    }
                    Some(total)
                } else {
                    response
                        .content_length()
                        .and_then(|length| usize::try_from(length).ok())
                };
                if total.is_some_and(|total| total > limit) {
                    return Err(CmdError::click(format!(
                        "Stado object API release GET response exceeds the {limit}-byte limit"
                    )));
                }
                if let Some(total) = total {
                    body.reserve(total.saturating_sub(body.capacity()));
                }

                let mut response = response;
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            if chunk.len() > limit.saturating_sub(body.len()) {
                                return Err(CmdError::click(format!(
                                    "Stado object API release GET response exceeds the \
                                     {limit}-byte limit"
                                )));
                            }
                            body.extend_from_slice(&chunk);
                        }
                        // The stream ending is not the object ending. The
                        // release route streams an unranged GET until its own
                        // window closes and then closes the body cleanly, with
                        // no length declared and no error to read: the darwin
                        // archive of 0.13.46 is 73,864,632 bytes and two reads
                        // of it here returned 22,925,186 and 15,318,446, both
                        // as `Ok`. A short object with a successful exit is
                        // worse than a failed download, because every caller
                        // downstream -- digest verification, archive extract,
                        // a host staging a release -- reports its own true
                        // finding about bytes that were never the object.
                        //
                        // Whole is provable only against a declared total, so
                        // that is the only thing accepted here. Anything else
                        // goes to the bounded byte-range reader below, which
                        // asks for one chunk at a time and knows the total
                        // from every `Content-Range` it gets back.
                        Ok(None) if total == Some(body.len()) => return Ok(body),
                        Ok(None) => return self.get_release_in_ranges(origin).await,
                        Err(error) => {
                            last_read_error = Some(format!(
                                "Stado object API release GET response body connection closed \
                                 before completion after {} bytes: {error}",
                                body.len()
                            ));
                            break;
                        }
                    }
                }
                break;
            }
            if recovery == 3 {
                break;
            }
        }
        Err(CmdError::click(last_read_error.unwrap_or_else(|| {
            "release GET exceeded three redirects".to_string()
        })))
    }

    /// Ask the release channel itself whether it serves one object.
    ///
    /// `stat` otherwise answers from the configured job store, and for a
    /// `stado://releases/...` URI that is the wrong witness entirely: the channel
    /// publishes through this route, and the local store has never held those bytes.
    /// Reading its silence as `absent` is how a baseline naming a published artifact
    /// gets certified against a store that could not have served it either way.
    ///
    /// Five states, because two would let silence pass for absence and three let
    /// every kind of silence pass for one kind. A redirect counts as present: this
    /// route answers a served object by redirecting to where the bytes live, and the
    /// client does not follow it, so the redirect IS the testimony. An explicit 404
    /// is absence. Every other status is a way of not answering, and
    /// [`unanswered_for_status`] says which way, because `401` (this reader may not
    /// ask), `503` (the boundary is down, ask again) and `502` (the resolver's SSH
    /// forward carried nothing) have three different remedies and used to arrive as
    /// one word. A transport error that never produced a status answered nothing at
    /// all, so it is unreachable outright.
    async fn stat_release(&self, uri: &str) -> Result<Presence, CmdError> {
        let endpoint = self.endpoint("/api/release/object", &[("uri", uri)])?;
        match self.request(reqwest::Method::GET, endpoint).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status.is_redirection() {
                    let size = response
                        .content_length()
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or_default();
                    Ok(Presence::Present {
                        size,
                        version: None,
                        detail: None,
                    })
                } else if status == reqwest::StatusCode::NOT_FOUND {
                    Ok(Presence::Absent)
                } else {
                    Ok(unanswered_for_status(
                        status.as_u16(),
                        format!("the release channel answered HTTP {status}"),
                    ))
                }
            }
            Err(error) => Ok(Presence::Unreachable(error.to_string())),
        }
    }

    async fn list(&self, namespace: &str, prefix: &str) -> Result<Vec<Value>, CmdError> {
        let endpoint = self.endpoint(
            "/api/object/list",
            &[("namespace", namespace), ("prefix", prefix)],
        )?;
        let bearer = self.release_bearer_for(namespace, prefix).await?;
        let response = self
            .request_as(reqwest::Method::GET, endpoint, bearer.as_deref())
            .send()
            .await?;
        let payload: RemoteObjectListResponse = self
            .response_json(response, "object list", bearer.as_deref())
            .await?;
        let mut values = Vec::with_capacity(payload.objects.len());
        for item in payload.objects {
            let object = crate::object_store::ObjectRef::parse(&item.uri).map_err(|error| {
                CmdError::click(format!(
                    "Stado object API returned an invalid object-list URI: {error}"
                ))
            })?;
            // Two different faults, and they were one refusal. An item whose
            // `uri`, `namespace` and `key` disagree is a broken store and
            // must stop the read. An item outside the requested prefix is a
            // gateway that answered a wider question than it was asked —
            // `prefix=queue/` returning `queue_priority/` — and the honest
            // response is to keep what was asked for, because a fleet still
            // running that gateway must not make this reader refuse a store
            // that holds exactly the right objects.
            if object.namespace() != namespace
                || item.namespace.as_str() != object.namespace()
                || item.key.as_str() != object.key()
            {
                return Err(CmdError::click(
                    "Stado object API returned an inconsistent object-list item",
                ));
            }
            if !object.key().starts_with(prefix) {
                continue;
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
        let bearer = self.release_bearer(uri).await?;
        let response = self
            .request_as(reqwest::Method::DELETE, endpoint, bearer.as_deref())
            .send()
            .await?;
        let payload: RemoteDeleteResponse = self
            .response_json(response, "object DELETE", bearer.as_deref())
            .await?;
        if payload.state != "absent" || payload.uri != uri {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object DELETE response",
            ));
        }
        Ok(())
    }

    fn generic_bearer(&self) -> Option<&str> {
        match &self.auth {
            RemoteObjectAuth::Generic(token) => Some(token),
            RemoteObjectAuth::PublisherOnly | RemoteObjectAuth::Public => None,
        }
    }

    async fn response_json<T>(
        &self,
        response: reqwest::Response,
        operation: &str,
        bearer: Option<&str>,
    ) -> Result<T, CmdError>
    where
        T: serde::de::DeserializeOwned,
    {
        let status = response.status();
        let body = self
            .success_body(response, max_object_api_json_body(), operation, bearer)
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
        bearer: Option<&str>,
    ) -> Result<Vec<u8>, CmdError> {
        if !response.status().is_success() {
            return Err(self.response_error(response, bearer).await);
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
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            CmdError::click(format!(
                "Stado object API {operation} response body connection closed before completion \
                 after {} bytes: {error}",
                body.len()
            ))
        })? {
            if chunk.len() > limit.saturating_sub(body.len()) {
                return Err(CmdError::click(format!(
                    "Stado object API {operation} response exceeds the {limit}-byte limit"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn response_error(
        &self,
        mut response: reqwest::Response,
        bearer: Option<&str>,
    ) -> CmdError {
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
                    let detail = response_body_detail(&body, self.generic_bearer(), bearer);
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
        let detail = response_body_detail(&body, self.generic_bearer(), bearer);
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

/// Ceiling on one whole object-API request, however large its body.
///
/// Sized to clear the largest transfer this client performs rather than to
/// express a latency expectation: a 70 MB release read-back over a relayed
/// tailnet path is legitimate and must not be cut, which is why the total
/// 60-second timeout that once lived here was removed. What this replaces is
/// not a slow request but an eternal one -- the caller that holds a lock, or
/// a fleet gate, while a request that will never return is still outstanding.
const OBJECT_REQUEST_CEILING: Duration = Duration::from_secs(900);

/// One HTTPS client that trusts what `storage.stado.ca_file` names.
///
/// The queue backend already loads that certificate; callers that built their
/// own client did not, so the moment the fleet's control plane moved from
/// loopback to a tailnet HTTPS origin they failed with "error sending
/// request" -- which reads like the host is down rather than like this
/// process was never told whom to trust.
///
/// Built once per process, per configuration. `reqwest::Client` owns the
/// connection pool, and this was called per object operation --
/// `RemoteObjectApi::configured()` runs on every get, stat, list, put,
/// get_versioned, put_if_version and delete, and the beacon publisher, the
/// doctor and the host-recovery release path each call it directly. A client
/// per call is a pool of one connection thrown away, which is how a store
/// ends up with 1,059 sockets in `TIME_WAIT` beside 41 established and an
/// object API pinned at 99.8% of a core, and a read that queues behind those
/// handshakes is the read that starves the agent loop. Same fix, and the same
/// reasoning, as `queue::stado_object::StadoObjectBackend::shared_client`.
///
/// Keyed by the inputs the client is built from -- the CA file and the
/// resolved origin hosts -- so a configuration change still produces a new
/// client rather than reusing one that trusts the wrong authority.
pub(crate) fn fleet_https_client() -> Result<reqwest::Client, CmdError> {
    static CLIENTS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, reqwest::Client>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = format!(
        "{}|{}",
        crate::config::wc_stado_storage_ca_file().trim(),
        configured_origin_hosts().join(",")
    );
    if let Some(client) = CLIENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return Ok(client.clone());
    }
    let client = build_fleet_https_client()?;
    CLIENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, client.clone());
    Ok(client)
}

fn build_fleet_https_client() -> Result<reqwest::Client, CmdError> {
    // Bound DNS/TCP establishment and an actually stalled body, not the total
    // lifetime of an active immutable transfer. The former total 60-second
    // timeout cut healthy 70 MB writer read-backs off at 42–56 MB; retries
    // restarted from byte zero and could therefore never satisfy publication.
    // A 60-second read timeout retains the fail-fast control-plane contract
    // while allowing a body that keeps making progress to finish.
    //
    // Those two bound a phase each and together still bounded nothing. On
    // 2026-09-03 three processes on charless-mac-mini were alive 9h34m, 9h58m
    // and 9h58m against this API, holding 11, 10 and 19 sockets, and one of
    // them held the disk janitor's exclusive run lock for its whole life --
    // so cleanup completed no pass, `disk_cleanup_stalled` latched, and the
    // host claimed nothing for the rest of the day. A connect that succeeds
    // and a body that trickles are both inside the two bounds above; a peer
    // that stops answering without ever sending FIN or RST is outside all of
    // them, and nothing here would ever have given up.
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        // A ceiling on the WHOLE request, so no single call can outlive the
        // work it was issued for. Generous on purpose: it has to clear the
        // largest immutable transfer this client performs, which is why the
        // old 60-second total was wrong. It is not a latency budget -- it is
        // the difference between a request that fails and one that never
        // returns, which is what a caller holding a lock cannot survive.
        .timeout(OBJECT_REQUEST_CEILING)
        // The same pool contract as
        // `queue::stado_object::StadoObjectBackend::client`, and for the same
        // reason: the object API holds a reused connection for 120 s
        // (`Dashboard::KEEP_ALIVE_IDLE`), so this side retires it first at
        // 90 s and never writes into a socket the server is closing. Eight
        // warm connections per host bound the idle set.
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        // Prove the peer is still there. A vanished peer leaves an
        // ESTABLISHED socket that reads forever, which is exactly what was
        // measured today; keep-alive probes turn that into an error the
        // caller can act on.
        .tcp_keepalive(Duration::from_secs(60));
    for host in configured_origin_hosts() {
        // The tailnet states where its own names live. Asking the system
        // resolver about a MagicDNS name is asking a witness that may not have
        // been told: on 2026-09-02 it answered the public `ts.net` front end
        // once and nothing the next time, while the tailnet address served the
        // same route in 82 ms. SNI and certificate validation still use the
        // name, so this decides the route and never the identity.
        if let Some(address) = crate::tailnet::address_of(&host) {
            builder = builder.resolve(&host, std::net::SocketAddr::new(address, 0));
        }
    }
    let ca_file = crate::config::wc_stado_storage_ca_file().trim().to_string();
    if !ca_file.is_empty() {
        let path = crate::config_file::expand_tilde(&ca_file);
        let pem = std::fs::read(&path).map_err(|error| {
            CmdError::click(format!(
                "cannot read storage.stado.ca_file {}: {error}",
                path.display()
            ))
        })?;
        let certificate = reqwest::Certificate::from_pem(&pem).map_err(|error| {
            CmdError::click(format!(
                "storage.stado.ca_file {} is not a PEM certificate: {error}",
                path.display()
            ))
        })?;
        builder = builder.add_root_certificate(certificate);
    }
    builder.build().map_err(CmdError::from)
}

/// Every host this process may address as a Stado HTTP origin.
///
/// Read from the accessors that own each origin rather than from raw
/// environment variables, so a value configured in `config.json` is pinned
/// exactly like one exported into the process. Malformed values are dropped
/// here and refused where they are used, because this function decides
/// routing and must never be the thing that rejects a configuration.
fn configured_origin_hosts() -> Vec<String> {
    let mut hosts = Vec::new();
    let candidates = [
        crate::config::stado_api_url(),
        crate::config::wc_stado_storage_url().to_string(),
        std::env::var("STADO_HOST_HEALTH_API_URL").unwrap_or_default(),
    ];
    for candidate in candidates {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        let Ok(url) = url::Url::parse(candidate) else {
            continue;
        };
        let Some(host) = url.host_str() else { continue };
        if crate::tailnet::is_magicdns_name(host) && !hosts.iter().any(|known| known == host) {
            hosts.push(host.to_string());
        }
    }
    hosts
}

fn validated_object_base_url(variable: &str, value: &str) -> Result<Option<url::Url>, CmdError> {
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

fn configured_object_base_url(variable: &str) -> Result<Option<url::Url>, CmdError> {
    let value = match std::env::var(variable) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(CmdError::click(format!("{variable} must be valid Unicode")));
        }
    };
    validated_object_base_url(variable, &value)
}

/// The canonical origin from the environment, and then from `api.url`.
///
/// `STADO_API_URL` is a configuration field, not an environment-only switch:
/// `config::stado_api_url` resolves both, and that is how the scheduler, the
/// doctor and every enrolment path read it. Reading the environment alone made
/// `stado host release` refuse a fleet delivery with "STADO_API_URL is
/// required for canonical release reads" on a host whose own configuration
/// declared the canonical origin — printed back by `host config-show` while
/// being refused.
///
/// Only the release channel resolves it this way. The private object plane
/// keeps its own endpoint: widening the shared reader instead sent every
/// object write to the public origin, and a source archive PUT there answered
/// `504 FUNCTION_INVOCATION_TIMEOUT` twice before the cause was the diff.
fn configured_api_origin() -> Result<Option<url::Url>, CmdError> {
    if let Some(url) = configured_object_base_url("STADO_API_URL")? {
        return Ok(Some(url));
    }
    validated_object_base_url("api.url", &crate::config::stado_api_url())
}

/// Canonical public origin for immutable release reads. Release consumers use
/// the same `STADO_API_URL` contract as `storage get|stat|url`; there is no
/// release-specific origin that can drift from it.
///
/// HTTPS is the rule because the origin leaves the machine asking. The one
/// exception is loopback HTTP: the delivery fetch runs on the target itself,
/// so a loopback origin can only name that host's own store — self-delivery,
/// with no network path to tamper with. [`plan`](crate::deploy::host_release::plan)
/// keeps the per-target gate: it accepts this shape only for the host the
/// service directory says serves the object API.
pub(crate) fn release_api_origin() -> Result<String, CmdError> {
    let url = configured_api_origin()?
        .ok_or_else(|| CmdError::click("STADO_API_URL is required for canonical release reads"))?;
    if url.scheme() != "https" && !crate::deploy::host_release::loopback_http_origin(url.as_str()) {
        return Err(CmdError::click(
            "STADO_API_URL must use HTTPS for delivery to fleet hosts; loopback HTTP is allowed \
             only when the target is its own release store",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub(crate) fn object_api_endpoint(
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

fn response_body_detail(
    body: &[u8],
    generic_bearer: Option<&str>,
    request_bearer: Option<&str>,
) -> String {
    let detail = String::from_utf8_lossy(body);
    let detail = detail.trim();
    if detail.is_empty() {
        return "<empty response body>".to_string();
    }
    let mut redacted = detail.to_string();
    for secret in [generic_bearer, request_bearer].into_iter().flatten() {
        if !secret.is_empty() {
            redacted = redacted.replace(secret, "[REDACTED]");
        }
    }
    redacted
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

#[derive(Debug, Clone)]
struct StoreObjectOutcome {
    uri: String,
    created: bool,
}

/// Store one object through whichever route its namespace requires, create-only
/// for `releases` whether or not the caller asks.
///
/// Shared with `stado release publish` so the publisher cannot drift from what
/// `storage put` does: one implementation of "how an object reaches the channel".
pub(crate) async fn store_object(
    uri: &str,
    source: &str,
    content_type: &str,
    if_absent: bool,
) -> Result<String, CmdError> {
    store_object_with_metadata(uri, source, content_type, if_absent, &BTreeMap::new()).await
}

pub(crate) async fn store_object_with_metadata(
    uri: &str,
    source: &str,
    content_type: &str,
    if_absent: bool,
    extra_metadata: &BTreeMap<String, String>,
) -> Result<String, CmdError> {
    Ok(
        store_object_with_metadata_outcome(uri, source, content_type, if_absent, extra_metadata)
            .await?
            .uri,
    )
}

async fn store_object_with_metadata_outcome(
    uri: &str,
    source: &str,
    content_type: &str,
    if_absent: bool,
    extra_metadata: &BTreeMap<String, String>,
) -> Result<StoreObjectOutcome, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    let create_only = if_absent || object.namespace() == "releases";
    let mut metadata = crate::object_store::metadata(&object, content_type);
    for (name, value) in extra_metadata {
        if !name.starts_with("stado-")
            || metadata.contains_key(name)
            || value.is_empty()
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err(CmdError::click(
                "custom object metadata must use unique non-empty stado-* fields",
            ));
        }
        metadata.insert(name.clone(), value.clone());
    }
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        let bytes = read_object_source(source)?;
        if create_only {
            match remote.get_optional(&uri).await? {
                Some(existing) if existing == bytes => {
                    return Ok(StoreObjectOutcome {
                        uri,
                        created: false,
                    })
                }
                Some(_) => {
                    return Err(CmdError::click(format!(
                        "immutable object already differs on the writer: {uri}"
                    )))
                }
                None => {}
            }
        }
        let expected_sha = Sha256::digest(&bytes);
        remote
            .put_with_metadata(&uri, content_type, create_only, bytes, extra_metadata)
            .await?;
        let stored = remote.get(&uri).await?;
        if Sha256::digest(&stored) != expected_sha {
            return Err(CmdError::click(format!(
                "object writer read-back differs immediately after PUT: {uri}"
            )));
        }
        return Ok(StoreObjectOutcome { uri, created: true });
    }
    let path = object.storage_path();
    let store = JobStorage::new().await?;
    let stdin_bytes = if create_only && source == "-" {
        Some(read_object_source(source)?)
    } else {
        None
    };
    let uploaded = if create_only {
        if let Some(bytes) = stdin_bytes.as_ref() {
            let mut staged = tempfile::NamedTempFile::new()?;
            staged.write_all(bytes)?;
            store.upload_file_if_absent(&path, staged.path()).await?
        } else {
            store
                .upload_file_if_absent(&path, std::path::Path::new(source))
                .await?
        }
    } else {
        let bytes = read_object_source(source)?;
        store.upload_bytes(&path, &bytes).await?;
        true
    };
    if !uploaded {
        let incoming = match stdin_bytes {
            Some(bytes) => bytes,
            None => read_object_source(source)?,
        };
        let existing = store.read_bytes(&path).await?.ok_or_else(|| {
            CmdError::click(format!(
                "{object} won create-only admission but is no longer readable"
            ))
        })?;
        if Sha256::digest(&existing) == Sha256::digest(&incoming) {
            return Ok(StoreObjectOutcome {
                uri,
                created: false,
            });
        }
        let policy = if object.namespace() == "releases" {
            "release objects are immutable"
        } else {
            "--if-absent refused to replace it"
        };
        return Err(CmdError::click(format!(
            "{object} already exists; {policy}"
        )));
    }
    store.backend().set_metadata(&path, &metadata).await?;
    Ok(StoreObjectOutcome { uri, created: true })
}

async fn put(args: &StoragePutArgs) -> Result<(), CmdError> {
    let outcome = store_object_with_metadata_outcome(
        &args.uri,
        &args.source,
        &args.content_type,
        args.if_absent,
        &BTreeMap::new(),
    )
    .await?;
    if args.json {
        let stored = fetch_object_from_writer(&outcome.uri).await?;
        echo_json(&serde_json::to_value(StoragePutReceipt {
            schema: "stado.storage-put-receipt.v1".into(),
            state: if outcome.created {
                "stored".into()
            } else {
                "replayed".into()
            },
            created: outcome.created,
            uri: outcome.uri,
            sha256: hex::encode(Sha256::digest(&stored)),
            bytes: stored.len(),
            content_type: args.content_type.clone(),
        })?)?;
    } else if outcome.created {
        println!("stored {}", outcome.uri);
    } else {
        println!("replayed {}", outcome.uri);
    }
    Ok(())
}

/// Fetch through the authenticated object writer route, including release
/// objects that are not yet visible through the public download facade.
pub(crate) async fn fetch_object_from_writer(uri: &str) -> Result<Vec<u8>, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote.get(&uri).await;
    }
    let store = JobStorage::new().await?;
    let Some(bytes) = store.read_bytes(&object.storage_path()).await? else {
        return Err(CmdError::click(format!("{object}: absent")));
    };
    Ok(bytes)
}

/// Fetch one object's bytes through whichever route its namespace requires.
/// Shared with `stado release publish` for the same reason as [`store_object`].
pub(crate) async fn fetch_object(uri: &str) -> Result<Vec<u8>, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if object.namespace() == "releases" {
        if let Some(remote) = RemoteObjectApi::configured_release_reader()? {
            return remote.get_release(&uri).await;
        }
    } else if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote.get(&uri).await;
    }
    let store = JobStorage::new().await?;
    let Some(bytes) = store.read_bytes(&object.storage_path()).await? else {
        return Err(CmdError::click(format!("{object}: absent")));
    };
    Ok(bytes)
}

/// Whether one object is actually there, routed exactly like
/// [`fetch_object`] but without moving the bytes.
///
/// Asking a whole version whether it is complete means asking about every
/// object in it, and one of those is a 72 MB archive; a presence question
/// must not download it.
///
/// An unanswered [`Presence`] propagates as an error rather than `false`,
/// because "the store did not answer" read as "the object is missing" is the
/// exact confusion [`Presence`] exists to keep visible — and here it would
/// turn a network blip into a false accusation that a good release is
/// half-published. The refusal carries WHICH kind of unanswered it was, so
/// the caller deciding whether a coordinate is spent is told whether to
/// repair a credential, wait, or chase the transport.
pub(crate) async fn release_object_present(uri: &str) -> Result<bool, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if object.namespace() == "releases" {
        if let Some(remote) = RemoteObjectApi::configured_release_reader()? {
            let presence = remote.stat_release(&uri).await?;
            if !presence.answered() {
                return Err(CmdError::click(format!(
                    "cannot tell whether {uri} is published — {}",
                    presence.unanswered_sentence(&uri)
                )));
            }
            return Ok(matches!(presence, Presence::Present { .. }));
        }
    }
    let store = JobStorage::new().await?;
    Ok(store.read_bytes(&object.storage_path()).await?.is_some())
}

/// Source identity that authorizes delivery for one release coordinate.
///
/// New releases are bound by the platformless version claim. Coordinates
/// published before that contract retain a validated platform-claim fallback.
pub(crate) async fn release_claim_source(
    product: &str,
    version: &str,
    platform: &str,
) -> Result<String, CmdError> {
    let version_base =
        crate::release_control::release_version_base(product, version).map_err(CmdError::click)?;
    let version_uri = format!(
        "{version_base}/{}",
        crate::release_control::RELEASE_VERSION_REVISION_NAME
    );
    if release_object_present(&version_uri).await? {
        let bytes = fetch_object(&version_uri).await?;
        let claim: crate::release_control::VersionRevision = serde_json::from_slice(&bytes)
            .map_err(|error| {
                CmdError::click(format!(
                    "{version_uri} is not a valid version claim: {error}"
                ))
            })?;
        if !claim.describes(product, version) {
            return Err(CmdError::click(format!(
                "{version_uri} does not describe {product}/{version}"
            )));
        }
        return Ok(claim.source_revision);
    }

    let platform_base = crate::release_control::release_base(product, version, platform)
        .map_err(CmdError::click)?;
    let platform_uri = format!(
        "{platform_base}/{}",
        crate::release_control::RELEASE_REVISION_NAME
    );
    let bytes = fetch_object(&platform_uri).await?;
    let claim: crate::release_control::CoordinateRevision = serde_json::from_slice(&bytes)
        .map_err(|error| {
            CmdError::click(format!(
                "{platform_uri} is not a valid platform claim: {error}"
            ))
        })?;
    if !claim.describes(product, version, platform) {
        return Err(CmdError::click(format!(
            "{platform_uri} does not describe {product}/{version}/{platform}"
        )));
    }
    Ok(claim.source_revision)
}

/// The exact byte count the release channel holds for one object.
///
/// The operator side knows this before the target does, and telling the target
/// is cheaper and more robust than making it discover the number: the host
/// script used to derive it from a `Range: 0-0` answer's `Content-Range`, and
/// the dashboard's own release route does not implement ranges — only the
/// tailnet proxy in front of it does. So a target fetching from the store it
/// serves itself, over loopback, got no `Content-Range` and refused with
/// `fetch no_declared_size` on 2026-09-03, while the same object read from any
/// other node answered `206 bytes 0-0/75433627`.
pub(crate) async fn release_object_size(uri: &str) -> Result<u64, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if object.namespace() == "releases" {
        if let Some(remote) = RemoteObjectApi::configured_release_reader()? {
            let presence = remote.stat_release(&uri).await?;
            if !presence.answered() {
                return Err(CmdError::click(format!(
                    "cannot read the published size of {uri} — {}",
                    presence.unanswered_sentence(&uri)
                )));
            }
            return match presence {
                Presence::Present { size, .. } => Ok(size as u64),
                _ => Err(CmdError::click(format!("{uri} is not published"))),
            };
        }
    }
    let store = JobStorage::new().await?;
    let bytes = store
        .read_bytes(&object.storage_path())
        .await?
        .ok_or_else(|| CmdError::click(format!("{uri} is not published")))?;
    Ok(bytes.len() as u64)
}

/// One `(version, platform)` coordinate the release channel holds, with the
/// object names it actually carries and when its claim was written.
///
/// The names come along because the audit's questions are about the SET, not
/// about any one object: "is this whole", "is this only a claim", "did two
/// publishers disagree". They are already in the listing this walk performs,
/// so carrying them costs nothing and saves the caller a second walk.
#[derive(Debug, Clone)]
pub(crate) struct PublishedCoordinate {
    pub version: String,
    pub platform: String,
    /// True for a synthetic entry representing a version claim that has no
    /// platform objects yet.
    pub version_scope: bool,
    /// Whether the version-scoped arbitration record exists for this version.
    pub has_version_claim: bool,
    /// Every object name directly under the coordinate prefix.
    pub names: BTreeSet<String>,
    /// When the version claim was written, falling back to the legacy
    /// platform claim for releases created before version-scoped arbitration.
    pub claim_written_at: Option<DateTime<Utc>>,
}

impl PublishedCoordinate {
    /// A coordinate that holds its claim and nothing else.
    ///
    /// The claim is written create-only BEFORE any artifact, by every
    /// publisher, so this is the state of a publication that stated which
    /// build it was and then wrote no bytes. It is not a partial coordinate:
    /// there is nothing to be short of yet, and `SHA256SUMS` — the object
    /// that declares what a complete coordinate holds — is exactly what is
    /// missing, so no object-level audit can say more than "absent".
    pub fn claim_only(&self) -> bool {
        let claim_name = if self.version_scope {
            crate::release_control::RELEASE_VERSION_REVISION_NAME
        } else {
            crate::release_control::RELEASE_REVISION_NAME
        };
        self.names.len() == 1 && self.names.contains(claim_name)
    }
}

/// Every coordinate the release channel actually holds for one product,
/// newest version first.
///
/// Derived from the store's own listing rather than from git tags. A tag is
/// created before publication and survives one that never completed, so a tag
/// list answers "what did someone intend" while this answers "what is there" —
/// and the gap between those two is where `stado/0.10.0/darwin-arm64` sat at 0
/// objects of 9 from April until it was found by accident.
pub(crate) async fn published_release_coordinates(
    product: &str,
) -> Result<Vec<PublishedCoordinate>, CmdError> {
    let prefix = format!("{product}/");
    // Keys and their timestamps, whichever store answers. The authenticated
    // list route when the object API is configured; the backend's own listing
    // otherwise, so the audit still runs on a host holding its releases
    // locally rather than reporting that it could not look.
    let keys: Vec<(String, Option<DateTime<Utc>>)> =
        match RemoteObjectApi::configured_for_list("releases", &prefix)? {
            Some(remote) => remote
                .list("releases", &prefix)
                .await?
                .into_iter()
                .filter_map(|entry| {
                    let key = entry.get("key").and_then(Value::as_str)?.to_string();
                    let updated = entry
                        .get("updated_at")
                        .and_then(Value::as_str)
                        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                        .map(|stamp| stamp.with_timezone(&Utc));
                    Some((key, updated))
                })
                .collect(),
            None => {
                let store = JobStorage::new().await?;
                let namespaced =
                    crate::object_store::ObjectRef::namespace_prefix("releases", &prefix)?;
                store
                    .backend()
                    .list_blobs_with_meta(&namespaced)
                    .await?
                    .into_iter()
                    .filter_map(|blob| {
                        crate::object_store::ObjectRef::from_storage_path(&blob.name)
                            .ok()
                            .map(|object| (object.key().to_string(), blob.updated))
                    })
                    .collect()
            }
        };
    let mut seen: BTreeMap<(String, String), PublishedCoordinate> = BTreeMap::new();
    let mut version_claims: BTreeMap<String, Option<DateTime<Utc>>> = BTreeMap::new();
    for (key, updated) in keys {
        let key = key.as_str();
        // Residue from an interrupted multipart upload is not a published
        // object. `stado storage put` stages parts below this suffix and only
        // promotes the complete object.
        if key.contains(".__stado_upload/") {
            continue;
        }
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() == 2 && parts[0] == product {
            let entry = seen
                .entry((parts[1].to_string(), String::new()))
                .or_insert_with(|| PublishedCoordinate {
                    version: parts[1].to_string(),
                    platform: String::new(),
                    version_scope: true,
                    has_version_claim: false,
                    names: BTreeSet::new(),
                    claim_written_at: None,
                });
            entry.names.insert("<version-root-object>".to_string());
            continue;
        }
        if parts.len() == 3 && parts[0] == product {
            if parts[2] == crate::release_control::RELEASE_VERSION_REVISION_NAME {
                version_claims.insert(parts[1].to_string(), updated);
            } else {
                let entry = seen
                    .entry((parts[1].to_string(), String::new()))
                    .or_insert_with(|| PublishedCoordinate {
                        version: parts[1].to_string(),
                        platform: String::new(),
                        version_scope: true,
                        has_version_claim: false,
                        names: BTreeSet::new(),
                        claim_written_at: None,
                    });
                entry.names.insert(parts[2].to_string());
            }
            continue;
        }
        // `<product>/<version>/<platform>/<name>`; anything shorter is not a
        // platform coordinate.
        if parts.len() < 4 || parts[0] != product {
            continue;
        }
        let name = parts[3..].join("/");
        let entry = seen
            .entry((parts[1].to_string(), parts[2].to_string()))
            .or_insert_with(|| PublishedCoordinate {
                version: parts[1].to_string(),
                platform: parts[2].to_string(),
                version_scope: false,
                has_version_claim: false,
                names: BTreeSet::new(),
                claim_written_at: None,
            });
        if name == crate::release_control::RELEASE_REVISION_NAME {
            entry.claim_written_at = updated;
        }
        entry.names.insert(name);
    }
    let versions_with_platforms: BTreeSet<String> = seen
        .values()
        .filter(|coordinate| !coordinate.version_scope)
        .map(|coordinate| coordinate.version.clone())
        .collect();
    for coordinate in seen.values_mut() {
        if !coordinate.version_scope {
            if let Some(written) = version_claims.get(&coordinate.version) {
                coordinate.has_version_claim = true;
                coordinate.claim_written_at = *written;
            }
        }
    }
    for (version, written) in version_claims {
        if versions_with_platforms.contains(&version) {
            continue;
        }
        let entry = seen
            .entry((version.clone(), String::new()))
            .or_insert_with(|| PublishedCoordinate {
                version,
                platform: String::new(),
                version_scope: true,
                has_version_claim: false,
                names: BTreeSet::new(),
                claim_written_at: None,
            });
        entry.has_version_claim = true;
        entry.claim_written_at = written;
        entry
            .names
            .insert(crate::release_control::RELEASE_VERSION_REVISION_NAME.to_string());
    }
    let mut coordinates: Vec<PublishedCoordinate> = seen.into_values().collect();
    coordinates.sort_by(|left, right| {
        if left.version == right.version {
            return left.platform.cmp(&right.platform);
        }
        // `version_newer(a, b)` is "b is newer than a", so this asks whether
        // `left` is the newer version and puts it first.
        if crate::release::version_newer(&right.version, &left.version) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    Ok(coordinates)
}

pub(crate) async fn fetch_object_versioned(
    uri: &str,
) -> Result<Option<(Vec<u8>, String)>, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    if object.namespace() == "releases" {
        return Err(CmdError::click(
            "release objects are immutable and have no catalog CAS path",
        ));
    }
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote.get_versioned(&object.to_string()).await;
    }
    let store = JobStorage::new().await?;
    Ok(store
        .read_text_versioned(&object.storage_path())
        .await?
        .map(|value| (value.content.into_bytes(), value.version)))
}

pub(crate) async fn compare_and_swap_object(
    uri: &str,
    content: &[u8],
    content_type: &str,
    expected_version: &str,
) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    if object.namespace() == "releases" {
        return Err(CmdError::click("release objects cannot be replaced"));
    }
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote
            .put_if_version(
                &object.to_string(),
                content_type,
                expected_version,
                content.to_vec(),
            )
            .await;
    }
    let text = std::str::from_utf8(content)
        .map_err(|_| CmdError::click("conditional object content must be UTF-8"))?;
    let store = JobStorage::new().await?;
    store
        .compare_and_swap_text(&object.storage_path(), expected_version, text)
        .await?;
    let metadata = crate::object_store::metadata(&object, content_type);
    store
        .backend()
        .set_metadata(&object.storage_path(), &metadata)
        .await?;
    Ok(())
}

pub(crate) async fn list_object_uris(
    namespace: &str,
    prefix: &str,
) -> Result<Vec<String>, CmdError> {
    let storage_prefix = crate::object_store::ObjectRef::namespace_prefix(namespace, prefix)?;
    if let Some(remote) = RemoteObjectApi::configured_for_list(namespace, prefix)? {
        return remote
            .list(namespace, prefix)
            .await?
            .into_iter()
            .map(|value| {
                value
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| CmdError::click("object list entry omitted uri"))
            })
            .collect();
    }
    let store = JobStorage::new().await?;
    let mut uris = Vec::new();
    for blob in store
        .backend()
        .list_blobs_with_meta(&storage_prefix)
        .await?
    {
        uris.push(crate::object_store::ObjectRef::from_storage_path(&blob.name)?.to_string());
    }
    uris.sort();
    Ok(uris)
}

async fn get(args: &StorageGetArgs) -> Result<(), CmdError> {
    let bytes = fetch_object(&args.uri).await?;
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
    let values = if let Some(remote) =
        RemoteObjectApi::configured_for_list(&args.namespace, &args.prefix)?
    {
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
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
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
    let base_url = configured_api_origin()?
        .ok_or_else(|| CmdError::click("STADO_API_URL is required to render an object URL"))?;
    let route = if object.namespace() == "releases" {
        "/api/release/object"
    } else {
        "/api/object"
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
    let body_mismatches: usize = diffs.iter().map(|diff| diff.body_mismatches.len()).sum();
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
