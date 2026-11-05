//! Build source snapshots in this host's object store.
//!
//! Every `stado build submit` and `stado release submit` publishes the
//! committed tree it builds as one create-only object,
//! `stado://sources/<product>/<sha256>/source.tar.gz`, and nothing removed
//! one: on the host serving the fleet's store they grew to tens of GiB, one
//! per build, and no cleaner covered them.
//!
//! Under the disk-full rule this cleaner removes a snapshot nothing names any
//! more, and keeps:
//!
//! - every snapshot a release catalog entry names (`system/release-catalog`),
//!   since a catalog entry is what a later build is made from;
//! - every snapshot a queued or running job names (`queue/`, `running/` of
//!   every namespace), since its worker still fetches it;
//! - every snapshot a release, build or quality record names whose `state` is
//!   not finished (`runs/release-pipeline*`, `runs/build`, `runs/quality`), and
//!   every snapshot a record without a readable state names;
//! - each product's newest snapshot, which the next build of an unchanged
//!   tree reuses rather than publishing again.
//!
//! Pinned dependency inputs (`sources/<product>/dependencies/...`) are never
//! touched: a release manifest names them, and `stado release catalog input
//! retire` is their remover. A record that cannot be read keeps every
//! snapshot, since it might name any of them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "source_snapshots";

/// The object store namespace build sources are published in.
const SOURCES: &str = "sources";
/// The file name every build source snapshot carries.
const SNAPSHOT: &str = "source.tar.gz";
/// The run record directories whose records name a source, under a
/// namespace's `runs/`.
const RECORD_DIRECTORIES: &[&str] = &[
    "release-pipeline",
    "release-pipeline-unfinished",
    "build",
    "quality",
];

/// The local store's per-namespace root on this host.
fn ecosystem(home: &Path) -> PathBuf {
    home.join(crate::config_file::LOCAL_STORAGE_ROOT)
        .join("ecosystem")
}

/// Every `stado://sources/...` string anywhere in `value`.
fn named(value: &Value, into: &mut BTreeSet<String>) {
    match value {
        Value::String(text) if text.starts_with("stado://sources/") => {
            into.insert(text.clone());
        }
        Value::Array(items) => items.iter().for_each(|item| named(item, into)),
        Value::Object(fields) => fields.values().for_each(|field| named(field, into)),
        _ => {}
    }
}

/// Every JSON file under `root`, `depth` levels deep at most.
fn json_files(root: &Path, depth: usize) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, level)) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("list {}: {error}", directory.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|error| format!("list {}: {error}", directory.display()))?;
            let kind = entry
                .file_type()
                .map_err(|error| format!("read {}: {error}", entry.path().display()))?;
            if kind.is_dir() && level < depth {
                pending.push((entry.path(), level + 1));
            } else if kind.is_file() && entry.file_name().to_string_lossy().ends_with(".json") {
                found.push(entry.path());
            }
        }
    }
    Ok(found)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))
}

/// Every source URI a catalog entry, a live job or an unfinished record
/// names, or why the names cannot all be read.
fn pinned(ecosystem: &Path) -> Result<BTreeSet<String>, String> {
    let mut pins = BTreeSet::new();
    for path in json_files(&ecosystem.join("system").join("release-catalog"), 3)? {
        named(&read_json(&path)?, &mut pins);
    }
    let namespaces = match std::fs::read_dir(ecosystem) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(pins),
        Err(error) => return Err(format!("list {}: {error}", ecosystem.display())),
    };
    for namespace in namespaces {
        let namespace =
            namespace.map_err(|error| format!("list {}: {error}", ecosystem.display()))?;
        let root = namespace.path();
        for prefix in crate::queue::control::WATCHED_PREFIXES {
            for path in json_files(&root.join(prefix), 2)? {
                named(&read_json(&path)?, &mut pins);
            }
        }
        for directory in RECORD_DIRECTORIES {
            for path in json_files(&root.join("runs").join(directory), 4)? {
                let record = read_json(&path)?;
                let finished = record
                    .get("state")
                    .and_then(Value::as_str)
                    .and_then(crate::release_pipeline::ReleaseRunState::named)
                    .is_some_and(|state| state.finished());
                if !finished {
                    named(&record, &mut pins);
                }
            }
        }
    }
    Ok(pins)
}

/// One snapshot on disk: its URI, path, size and modification time.
struct Snapshot {
    uri: String,
    path: PathBuf,
    bytes: u64,
    modified: std::time::SystemTime,
}

/// Every `sources/<product>/<sha256>/source.tar.gz` on disk, by product.
fn snapshots(ecosystem: &Path) -> Result<BTreeMap<String, Vec<Snapshot>>, String> {
    let root = ecosystem.join(SOURCES);
    let mut found: BTreeMap<String, Vec<Snapshot>> = BTreeMap::new();
    let products = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(format!("list {}: {error}", root.display())),
    };
    for product in products {
        let product = product.map_err(|error| format!("list {}: {error}", root.display()))?;
        let name = product.file_name().to_string_lossy().to_string();
        let Ok(digests) = std::fs::read_dir(product.path()) else {
            continue;
        };
        for digest in digests.flatten() {
            let sha = digest.file_name().to_string_lossy().to_string();
            // Only a 64-hex directory is a snapshot's; `dependencies/` and
            // anything else is a pinned input, never this cleaner's.
            if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                continue;
            }
            let path = digest.path().join(SNAPSHOT);
            let Ok(info) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !info.is_file() {
                continue;
            }
            found.entry(name.clone()).or_default().push(Snapshot {
                uri: format!("stado://{SOURCES}/{name}/{sha}/{SNAPSHOT}"),
                path,
                bytes: info.len(),
                modified: info.modified().unwrap_or(std::time::UNIX_EPOCH),
            });
        }
    }
    Ok(found)
}

/// Remove every snapshot nothing names, under the rule.
pub fn scan_source_snapshots(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let mut skipped: BTreeMap<String, i64> = BTreeMap::new();
    let mut kept_bytes: BTreeMap<String, i64> = BTreeMap::new();
    let mut keep = |reason: &str, bytes: u64| {
        *skipped.entry(reason.to_string()).or_default() += 1;
        *kept_bytes.entry(reason.to_string()).or_default() += bytes as i64;
    };
    let ecosystem = ecosystem(home);
    let found = match snapshots(&ecosystem) {
        Ok(found) => found,
        Err(error) => {
            report.add_error(CLEANER, &super::JanitorError::os(&error));
            report.source_snapshots = record;
            return;
        }
    };
    let pins = pinned(&ecosystem);
    let mut eligible: Vec<Snapshot> = Vec::new();
    for (_product, mut product_snapshots) in found {
        product_snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.modified));
        record.scanned_items += product_snapshots.len() as i64;
        let mut rest = product_snapshots.into_iter();
        if let Some(newest) = rest.next() {
            keep("newest_of_product", newest.bytes);
        }
        for snapshot in rest {
            match &pins {
                Err(_) => keep("records_unreadable", snapshot.bytes),
                Ok(pins) if pins.contains(&snapshot.uri) => keep("named_by_record", snapshot.bytes),
                Ok(_) => eligible.push(snapshot),
            }
        }
    }
    if let Err(error) = &pins {
        report.add_error(
            CLEANER,
            &super::JanitorError::os(&format!(
                "a record that may name a build source cannot be read, so every snapshot is kept: {error}"
            )),
        );
    }
    record.eligible_items = eligible.len() as i64;
    record.expected_bytes = eligible.iter().map(|snapshot| snapshot.bytes as i64).sum();
    if enforcing {
        for snapshot in eligible {
            match std::fs::remove_file(&snapshot.path) {
                Ok(()) => {
                    record.deleted_items += 1;
                    record.actual_free_delta_bytes += snapshot.bytes as i64;
                    // The digest directory held only the snapshot; an empty
                    // one is removed, a non-empty one left as it is.
                    if let Some(parent) = snapshot.path.parent() {
                        let empty = std::fs::read_dir(parent)
                            .map(|mut entries| entries.next().is_none())
                            .unwrap_or(false);
                        if empty {
                            if let Err(error) = std::fs::remove_dir(parent) {
                                report.add_error(
                                    CLEANER,
                                    &super::JanitorError::os(&format!(
                                        "cannot remove the emptied {}: {error}",
                                        parent.display()
                                    )),
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    keep("deletion_refused", snapshot.bytes);
                    report.add_error(
                        CLEANER,
                        &super::JanitorError::os(&format!(
                            "cannot remove {}: {error}",
                            snapshot.path.display()
                        )),
                    );
                }
            }
        }
    }
    record.skipped = skipped;
    record.skipped_bytes = kept_bytes;
    report.source_snapshots = record;
}
