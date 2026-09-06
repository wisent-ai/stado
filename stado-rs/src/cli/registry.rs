//! `stado registry validate|push|pull|self|doctor|host add|beacon-age` —
//! canonical registry management.
//!
//! `validate`, `push` and `pull` port the `registry` group of
//! `stado/cli.py`. `self`, `doctor`, `host add` and `beacon-age` have NO
//! Python original: they close items fifteen through seventeen of
//! `stado.wisent.com/docs/missing-commands`, written after the 2026-07-24
//! control-host incident, where the registry declared a host that
//! nothing on the box was honouring and no command could say so.
//!
//! Every read and write goes through [`targets::RegistryStore`], so the
//! group repairs the registry on whichever store `WC_STORAGE_BACKEND`
//! selects. It used to hardcode `gs://wisent-compute/registry.json` and
//! build a `GcsBackend` directly, which on an Azure-only deployment meant
//! the one document the coordinator's survival check reads
//! (`targets::fetch_registry_remote`) could be repaired by nobody.
//!
//! [`doctor`] and [`beacon_age`] source liveness from the host beacons
//! (`monitor/host_health.rs`) and the capacity broadcasts
//! (`queue/capacity.rs`), never from ssh, so they cost one prefix listing
//! and are safe to run on a loop.
//!
//! `push` compare-and-swaps, but until `--if-generation` existed it could
//! only swap against the generation it had just read itself, which is no
//! condition at all: a file carries no provenance, so a document edited
//! against generation 9 and pushed after somebody published 10 landed on top
//! of 10 with every guard here satisfied. The token can now come from the
//! caller — `pull --generation-only` hands it out, `push --if-generation`
//! spends it — so the read the operator's edit was made against is the read
//! the write is conditional on. A refused write is
//! [`REGISTRY_CONFLICT_EXIT`], never the generic failure code, so a reconcile
//! loop can re-read and re-apply instead of forcing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::capabilities::{Consumer, DeclarationSurface, DeclaredField, SiblingCondition};
use crate::deploy::service;
use crate::monitor::host_health;
use crate::queue::{capacity, JobStorage, StorageError};
use crate::targets::{
    self, bundled_registry_path, validate_registry_file, ComputeTarget, Registry, RegistryStore,
};

use super::table;
use super::CmdError;

/// The state a live launchd/systemd unit reports
/// (`deploy/host_health_beacon_macos.sh`, `deploy/host_health_beacon.sh`).
const ACTIVE_STATE: &str = "active";
/// A successful timer-triggered oneshot with an active native trigger.
///
/// This is intentionally distinct from [`ACTIVE_STATE`]: it proves scheduled
/// lifecycle health, not a continuously running process.
const SCHEDULED_STATE: &str = "scheduled";

/// Exit code for a registry write refused because the document had already
/// moved: `sysexits.h`'s `EX_TEMPFAIL`, "try again".
///
/// A reconcile loop has to tell "somebody wrote first, so re-read and
/// re-apply" from "the store is broken" without reading English. Both were
/// [`super::CLICK_ERROR_CODE`], so a loop either treated a lost race as fatal
/// or retried a genuine outage forever. Storage and validation failures keep
/// exit 1; only a lost condition is 75, and [`super::main_entry`] passes any
/// code other than 1 through unremapped.
pub const REGISTRY_CONFLICT_EXIT: i32 = 75;

/// How many times [`commit_document`] re-reads and re-applies a pure
/// transform before it hands the conflict back.
///
/// Bounded because an unbounded retry against a document some other loop is
/// rewriting every second is a command that never returns. Sixteen rounds
/// outlast every burst this fleet has produced; past that the contention is
/// the thing to report, not to sit inside.
const COMMIT_ROUNDS: usize = 16;

/// What the canonical object turned out to be when a conditional write was
/// refused — the one input the operator sentence, the typed receipt and the
/// exit code are all derived from.
enum RegistryActual {
    /// The object is there, at a generation that is not the caller's.
    Generation(String),
    /// There is no object at all, so no token can match one.
    Absent,
    /// The generation matched when it was read and had moved by the swap. The
    /// generation it carries now is whatever the winning writer produced, and
    /// this command deliberately does not go back to read it: that answer
    /// would be one more race, and the caller has to re-read anyway.
    Raced,
}

/// A refused conditional write, kept as data until the caller decides which
/// of its faces it needs.
///
/// One place produces all three — the sentence on stderr, the `conflict`
/// receipt on stdout and [`REGISTRY_CONFLICT_EXIT`] — so a machine reading
/// the receipt and an operator reading the sentence can never disagree about
/// what happened.
struct RegistryConflict {
    location: String,
    expected: String,
    actual: RegistryActual,
}

impl RegistryConflict {
    /// The generation the object carries instead, when it is a generation at
    /// all: the receipt's `actual_generation`.
    fn actual_generation(&self) -> Option<&str> {
        match &self.actual {
            RegistryActual::Generation(version) => Some(version),
            RegistryActual::Absent | RegistryActual::Raced => None,
        }
    }

    /// The operator sentence and the machine-recognizable exit code.
    fn error(&self) -> CmdError {
        let observed = match &self.actual {
            RegistryActual::Generation(version) => {
                format!("{} is at generation {version}", self.location)
            }
            RegistryActual::Absent => format!("there is no document at {}", self.location),
            RegistryActual::Raced => format!(
                "{} moved between this command's read and its write",
                self.location
            ),
        };
        CmdError {
            message: Some(format!(
                "registry write refused: it is conditional on generation {} and {observed}. \
                 Another writer got there first, so applying this document would erase what \
                 they published. Re-read the registry (`stado registry pull --with-generation`), \
                 re-apply the change to what it now says, and write again with the new token. \
                 Exit {REGISTRY_CONFLICT_EXIT} means exactly this and nothing else: the store \
                 is healthy and the document is valid.",
                self.expected
            )),
            code: REGISTRY_CONFLICT_EXIT,
            ..CmdError::default()
        }
    }
}

/// Why a registry write did not happen, split so [`push`] can answer a lost
/// condition with a receipt and everything else with the failure it is.
enum RegistryWriteError {
    Conflict(RegistryConflict),
    Failed(CmdError),
}

impl From<RegistryWriteError> for CmdError {
    fn from(error: RegistryWriteError) -> Self {
        match error {
            RegistryWriteError::Conflict(conflict) => conflict.error(),
            RegistryWriteError::Failed(error) => error,
        }
    }
}

/// `stado registry push --json`, for every outcome including the refusal.
///
/// A caller that has to scrape "pushed ... generation=..." out of a sentence
/// to learn whether its edit landed is a caller that will one day mistake a
/// refusal for a success. `expected_generation` is the caller's own token or
/// null; `actual_generation` is what the object carried instead and is only
/// ever set on a `conflict`; `generation` and `replaced` are only ever set on
/// a `pushed`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryPushReceipt {
    schema: String,
    state: String,
    location: String,
    expected_generation: Option<String>,
    actual_generation: Option<String>,
    generation: Option<String>,
    replaced: Option<String>,
}

/// `stado registry pull --with-generation`: the document and the token that
/// makes it writable back, from one read.
///
/// Two reads cannot produce this object safely — the generation would belong
/// to a different document than the one printed beside it, which is the exact
/// lost update `--if-generation` exists to refuse.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryPullReceipt {
    schema: String,
    location: String,
    generation: String,
    document: Value,
}

fn source_path(path: Option<String>) -> PathBuf {
    path.map(PathBuf::from)
        .unwrap_or_else(bundled_registry_path)
}

pub fn validate(path: Option<String>) -> Result<(), CmdError> {
    let source = source_path(path);
    validate_registry_file(&source).map_err(|exc| CmdError::click(exc.to_string()))?;
    println!("valid registry: {}", source.display());
    Ok(())
}
fn import_names(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

fn render_import_receipt(receipt: &crate::registry_import::RegistryImportReceipt) {
    println!(
        "registry import: {}{}",
        receipt.state,
        receipt
            .generation
            .as_deref()
            .map(|generation| format!(" (generation {generation})"))
            .unwrap_or_default()
    );
    println!(
        "  imported hosts: {}",
        import_names(&receipt.imported_targets)
    );
    println!(
        "  unchanged hosts: {}",
        import_names(&receipt.unchanged_targets)
    );
    println!(
        "  imported fleets: {}",
        import_names(&receipt.imported_fleets)
    );
    println!(
        "  unchanged fleets: {}",
        import_names(&receipt.unchanged_fleets)
    );
    println!(
        "  imported sections: {}",
        import_names(&receipt.imported_sections)
    );
    for conflict in &receipt.conflicts {
        println!("  conflict: {}: {}", conflict.path, conflict.reason);
    }
    for rejection in &receipt.rejected {
        println!("  rejected: {rejection}");
    }
}

/// Additively adopt an existing registry-v2 file into the canonical registry.
///
/// Both this command and `POST /api/registry/import` call
/// [`crate::registry_import::import_bytes`]. The operation validates the whole
/// source before opening the destination, preserves every destination-only
/// field, refuses differing records, and verifies the conditional write before
/// returning an accepted receipt.
pub async fn import(path: String, json_output: bool) -> Result<(), CmdError> {
    let source = PathBuf::from(&path);
    let bytes = std::fs::read(&source).map_err(|error| {
        CmdError::click(format!(
            "cannot read registry import {}: {error}",
            source.display()
        ))
    })?;
    let receipt = crate::registry_import::import_bytes(&bytes)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        render_import_receipt(&receipt);
    }
    if receipt.accepted() {
        super::onboarding::record_registry_import_accepted(&receipt);
        return Ok(());
    }
    let detail = receipt
        .rejected
        .first()
        .cloned()
        .or_else(|| {
            receipt
                .conflicts
                .first()
                .map(|conflict| format!("{}: {}", conflict.path, conflict.reason))
        })
        .unwrap_or_else(|| "the source was not accepted".to_string());
    Err(CmdError {
        message: Some(format!("registry import {}: {detail}", receipt.state)),
        code: if receipt.state == "conflict" {
            REGISTRY_CONFLICT_EXIT
        } else {
            super::CLICK_ERROR_CODE
        },
        ..CmdError::default()
    })
}

/// Top-level keys the outgoing document would delete from the object that is
/// already there.
///
/// A registry write is a whole-document replace, so a caller holding a stale
/// or differently-modelled copy silently deletes every key its own model does
/// not know about. That is not hypothetical: on 2026-08-04 the canonical
/// document lost `channels`, `enrollment` and `fleets` between one read and
/// the next, and gained a `service_directory` block no checkout in the tree
/// modelled at the time — divergent builds writing the same object, each
/// erasing what it could not name. `targets::Registry` now keeps unmodelled
/// top-level keys in `extra`, and `fetch_document` hands read-modify-write
/// callers the raw document; this is the backstop for a payload that came
/// from neither.
///
/// Only removals are reported. Additions are how the document grows, and a
/// changed value is an edit rather than a loss.
fn removed_top_level_keys(current: &str, payload: &str) -> Vec<String> {
    let (Ok(Value::Object(before)), Ok(Value::Object(after))) = (
        serde_json::from_str::<Value>(current),
        serde_json::from_str::<Value>(payload),
    ) else {
        return Vec::new();
    };
    before
        .keys()
        .filter(|key| !after.contains_key(*key))
        .cloned()
        .collect()
}

/// The service directory's own publication counter, when the document carries
/// one.
///
/// `ServiceDirectory::generation` is what a consumer compares against the copy
/// it cached, and `ServiceDirectoryError::Stale` is the answer it gets when its
/// copy is older. That check only means something if the number never goes
/// backwards at the authority.
fn service_directory_generation(text: &str) -> Option<u64> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("service_directory")?
        .get("generation")?
        .as_u64()
}

/// The service directory itself, with its counter removed, so two documents
/// can be compared for whether the DECLARATIONS differ independently of the
/// number that is supposed to announce that they do.
fn service_directory_body(text: &str) -> Option<Value> {
    let mut directory = serde_json::from_str::<Value>(text)
        .ok()?
        .get("service_directory")?
        .clone();
    directory.as_object_mut()?.remove("generation");
    Some(directory)
}

/// The number of targets a registry document declares, or `None` when the
/// text is not a document with a `targets` array.
fn target_count(text: &str) -> Option<usize> {
    Some(
        serde_json::from_str::<Value>(text)
            .ok()?
            .get("targets")?
            .as_array()?
            .len(),
    )
}

/// The upload half of [`push`]: read the current generation, refuse the write
/// when it is not the one the caller made its edit against, refuse a write
/// that would delete a top-level key unless the operator said so,
/// compare-and-swap, then read back and verify BOTH the generation and the
/// bytes. Returns `(generation, previous_generation)`.
///
/// `expected_generation` is the caller's own token, from an earlier
/// [`pull`]. When it is `Some` the object must exist AND be at exactly that
/// generation, checked before any guard runs and before anything is written,
/// and the swap spends that token rather than the one this function just
/// read: a document edited against generation 9 cannot land on top of 10.
/// When it is `None` the swap is against the generation read here, which
/// only rules out a writer that lands between this read and this write.
///
/// `payload` is written verbatim, so [`push`] still uploads the operator's
/// exact file bytes rather than a re-serialization of them.
///
/// `allow_empty_fleet` is deliberately NOT `--force`: see the floor below.
async fn upload_payload(
    payload: &str,
    allow_removals: bool,
    allow_empty_fleet: bool,
    expected_generation: Option<&str>,
) -> Result<(String, String), RegistryWriteError> {
    let store = RegistryStore::open().await.map_err(|exc| {
        RegistryWriteError::Failed(CmdError::click(format!("registry upload failed: {exc}")))
    })?;
    let current = store.read_versioned().await.map_err(|exc| {
        RegistryWriteError::Failed(CmdError::click(format!("registry upload failed: {exc}")))
    })?;
    // Ahead of every guard and every write: a caller whose token no longer
    // names the canonical document is holding an edit to a document that no
    // longer exists, and the guards below cannot see that. They compare this
    // payload against whatever is there now, which is exactly the comparison
    // that passes while the write erases a publication the payload predates.
    if let Some(expected) = expected_generation {
        let actual = match current.as_ref() {
            Some(blob) if blob.version == expected => None,
            Some(blob) => Some(RegistryActual::Generation(blob.version.clone())),
            None => Some(RegistryActual::Absent),
        };
        if let Some(actual) = actual {
            return Err(RegistryWriteError::Conflict(RegistryConflict {
                location: store.location().to_string(),
                expected: expected.to_string(),
                actual,
            }));
        }
    }
    let previous_generation = current
        .as_ref()
        .map(|blob| blob.version.clone())
        .unwrap_or_else(|| "0".to_string());
    refuse_unsafe_replace(current.as_ref(), payload, allow_removals, allow_empty_fleet)
        .map_err(RegistryWriteError::Failed)?;
    let generation = match current {
        Some(blob) => {
            // The caller's token when it brought one, this read's generation
            // otherwise. They are equal here — the check above proved it — and
            // spending the caller's own token is what makes the write
            // conditional on the read the edit was made against.
            let token = expected_generation.unwrap_or(blob.version.as_str());
            match store.compare_and_swap(token, payload).await {
                Ok(generation) => generation,
                // The document moved between this function's read and its
                // swap. Same answer as a stale `--if-generation`, because it
                // is the same lost update seen a few milliseconds later.
                Err(StorageError::StorageConflict(_)) => {
                    return Err(RegistryWriteError::Conflict(RegistryConflict {
                        location: store.location().to_string(),
                        expected: token.to_string(),
                        actual: RegistryActual::Raced,
                    }));
                }
                Err(exc) => {
                    return Err(RegistryWriteError::Failed(CmdError::click(format!(
                        "registry upload failed: {exc}"
                    ))));
                }
            }
        }
        None => {
            let created = store.create_if_absent(payload).await.map_err(|exc| {
                RegistryWriteError::Failed(CmdError::click(format!(
                    "registry upload failed: {exc}"
                )))
            })?;
            if !created {
                // Somebody created the object while this command was deciding
                // it was absent, so this write has no condition to stand on.
                return Err(RegistryWriteError::Conflict(RegistryConflict {
                    location: store.location().to_string(),
                    expected: expected_generation.unwrap_or("0").to_string(),
                    actual: RegistryActual::Raced,
                }));
            }
            store
                .read_versioned()
                .await
                .map_err(|exc| {
                    RegistryWriteError::Failed(CmdError::click(format!(
                        "registry upload failed: {exc}"
                    )))
                })?
                .ok_or_else(|| {
                    RegistryWriteError::Failed(CmdError::click(
                        "registry upload verification could not read the object",
                    ))
                })?
                .version
        }
    };
    let confirmed = store
        .read_versioned()
        .await
        .map_err(|exc| {
            RegistryWriteError::Failed(CmdError::click(format!("registry upload failed: {exc}")))
        })?
        .ok_or_else(|| {
            RegistryWriteError::Failed(CmdError::click(
                "registry upload verification could not read the object",
            ))
        })?;
    if confirmed.version != generation || confirmed.content != payload {
        return Err(RegistryWriteError::Failed(CmdError::click(
            "registry upload verification returned different bytes",
        )));
    }
    Ok((generation, previous_generation))
}

/// Every refusal a whole-document replace earns on its own contents, run
/// before anything is written and independently of whose generation the swap
/// will spend.
///
/// `--if-generation` answers "is this an edit to the document that is there";
/// these answer "is this a document worth having at all", and a caller with a
/// perfectly current token still gets refused by them.
fn refuse_unsafe_replace(
    current: Option<&crate::queue::VersionedText>,
    payload: &str,
    allow_removals: bool,
    allow_empty_fleet: bool,
) -> Result<(), CmdError> {
    if !allow_removals {
        if let Some(blob) = current {
            let removed = removed_top_level_keys(&blob.content, payload);
            if !removed.is_empty() {
                return Err(CmdError::click(format!(
                    "registry upload refused: it would delete the top-level key(s) {} \
                     that generation {} carries. A registry write replaces the whole \
                     document, so this is what a stale copy or a build that does not \
                     model those keys does to them. Re-pull, re-apply the edit, and \
                     push again; pass --force only if the deletion is the intent.",
                    removed.join(", "),
                    blob.version
                )));
            }
        }
        if let Some(blob) = current {
            // Same accident as the deleted-key guard above, one level in: the
            // whole document is replaced, so a writer holding an older copy
            // publishes its older directory over a newer one and every
            // consumer's staleness check silently starts agreeing with it.
            // Observed on 2026-08-12, when the directory went from generation
            // 10 back to 5 and two corrected endpoints reverted with it.
            if let (Some(before), Some(after)) = (
                service_directory_generation(&blob.content),
                service_directory_generation(payload),
            ) {
                if after < before {
                    return Err(CmdError::click(format!(
                        "registry upload refused: its service directory is generation \
                         {after} and the registry already carries {before}. The counter \
                         consumers use to detect a stale directory would go backwards, \
                         so every cached copy older than {before} would start looking \
                         current. Re-pull, re-apply the edit, and push again; pass \
                         --force only if publishing the older directory is the intent."
                    )));
                }
                // The same lost update one notch subtler, and the one that
                // actually happened. On 2026-09-01 a corrected brama endpoint
                // was published, and a writer holding a copy from before it
                // pushed its own directory back at the SAME generation. The
                // decrease guard above never fired, every consumer's
                // staleness check agreed with the reverted copy, and the
                // correction was gone with nothing recording that it had
                // been.
                //
                // `push --if-generation` now refuses that write outright, and
                // read-modify-write callers have always used `push_document_if`
                // and the store's real CAS. This guard is what still catches
                // the caller who brought no token at all: a file carries no
                // provenance on its own, so the directory states the rule for
                // itself -- changing a declaration means advancing the counter
                // that announces the change.
                //
                // Only a CHANGED directory is refused. A writer that leaves
                // it byte-identical -- `release promote` rewriting
                // `release_control`, every fleet and enrollment edit -- is
                // untouched.
                if after == before
                    && service_directory_body(&blob.content) != service_directory_body(payload)
                {
                    return Err(CmdError::click(format!(
                        "registry upload refused: it changes the service directory but leaves \
                         its generation at {after}, the number the registry already carries. \
                         Consumers compare that counter against the copy they cached, so a \
                         changed directory published under an unchanged one is invisible to \
                         every one of them -- and if your copy predates a correction, this \
                         write reverts it silently. Re-pull, re-apply the edit, advance \
                         service_directory.generation, and push again; pass --force only if \
                         publishing a changed directory under the same generation is the \
                         intent."
                    )));
                }
            }
        }
    }
    // The floor `--force` may not cross. Every other guard here answers "did
    // the caller mean to drop this?"; this one answers "is this a fleet at
    // all?", and no legitimate edit to a three-host registry leaves zero
    // targets. On 2026-09-01 a worker ran
    // `stado registry push --force < /tmp/registry_updated.json`: the command
    // takes a PATH, so stdin was never read, `source_path(None)` resolved to
    // the repository's bundled `data/registry.json` - 65 bytes,
    // `{"schema_version":2,"coordinators":[],"targets":[]}` - and `--force`
    // waved it past the deleted-key guard that had refused the first attempt.
    // The live document lost all three targets, all eighteen of the mini's
    // service declarations, and the `fleets`, `inference`,
    // `placement_profiles`, `release_control` and `service_directory` keys.
    // `stado service reap` then answered that the always-on Mac is not in the
    // canonical registry.
    if !allow_empty_fleet {
        if let Some(blob) = current {
            if let (Some(before), Some(after)) =
                (target_count(&blob.content), target_count(payload))
            {
                if before > 0 && after == 0 {
                    return Err(CmdError::click(format!(
                        "registry upload refused: generation {} declares {before} target(s) and \
                         this document declares none. A fleet does not shrink to zero by edit, \
                         so this is an empty or wrong file, not an intention - most often the \
                         bundled skeleton reached through a missing path argument. --force does \
                         NOT cross this floor: pass --allow-empty-fleet if erasing every target \
                         is genuinely what you mean.",
                        blob.version
                    )));
                }
            }
        }
    }
    Ok(())
}

/// `stado registry push PATH|- [--if-generation TOKEN] [--force]
/// [--allow-empty-fleet] [--json]` — upload an operator's document.
///
/// `if_generation` is the token an earlier [`pull`] handed back. With it the
/// write is conditional on the read the edit was made against, so a
/// concurrent publication is refused with [`REGISTRY_CONFLICT_EXIT`] instead
/// of overwritten; without it the write is only conditional on the read
/// [`upload_payload`] does itself, which is what every push did before the
/// flag existed.
pub async fn push(
    path: Option<String>,
    force: bool,
    allow_empty_fleet: bool,
    if_generation: Option<String>,
    json_output: bool,
) -> Result<(), CmdError> {
    // This command takes a PATH, and with no path it falls back to the
    // repository's bundled document. A caller who pipes a body is therefore
    // silently ignored and something else is uploaded in its place - which is
    // exactly how the bundled 65-byte skeleton reached the canonical registry
    // on 2026-09-01. Refuse the ambiguity rather than resolve it silently,
    // and let `-` mean stdin for a caller who meant to pipe.
    let from_stdin = path.as_deref() == Some("-");
    if !from_stdin && path.is_none() && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(CmdError::usage(
            "a document was piped to `registry push` but this command reads a PATH, so the \
             piped bytes would be ignored and the bundled registry uploaded instead. Pass the \
             file's path, or `-` to read stdin deliberately.",
        ));
    }
    let (source, payload) = if from_stdin {
        let mut body = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)?;
        (PathBuf::from("<stdin>"), body)
    } else {
        let source = source_path(path);
        let payload = std::fs::read_to_string(&source)?;
        (source, payload)
    };
    let document: Value = serde_json::from_str(&payload)
        .map_err(|exc| CmdError::click(format!("{}: {exc}", source.display())))?;
    // Ahead of every store call, as it has always been: a document that would
    // not validate never reaches the registry, whatever token it carries.
    warn_scoped_validation(validate_for_write(&document).await?);
    let location = targets::registry_location();
    match upload_payload(&payload, force, allow_empty_fleet, if_generation.as_deref()).await {
        Ok((generation, previous_generation)) => {
            if json_output {
                return print_push_receipt(&RegistryPushReceipt {
                    schema: PUSH_RECEIPT_SCHEMA.to_string(),
                    state: "pushed".to_string(),
                    location,
                    expected_generation: if_generation,
                    actual_generation: None,
                    generation: Some(generation),
                    replaced: Some(previous_generation),
                });
            }
            println!(
                "pushed {} -> {location} generation={generation} replaced={previous_generation}",
                source.display()
            );
            Ok(())
        }
        Err(RegistryWriteError::Conflict(conflict)) => {
            // The receipt goes out before the error, so a `--json` caller has
            // the two generations in hand no matter how it treats exit 75.
            if json_output {
                print_push_receipt(&RegistryPushReceipt {
                    schema: PUSH_RECEIPT_SCHEMA.to_string(),
                    state: "conflict".to_string(),
                    location: conflict.location.clone(),
                    expected_generation: Some(conflict.expected.clone()),
                    actual_generation: conflict.actual_generation().map(str::to_string),
                    generation: None,
                    replaced: None,
                })?;
            }
            Err(conflict.error())
        }
        // A storage or verification failure is not a conflict: nothing about
        // it says "re-read and re-apply", so it keeps exit 1 and emits no
        // receipt for a caller to mistake for a decision about generations.
        Err(RegistryWriteError::Failed(error)) => Err(error),
    }
}

const PUSH_RECEIPT_SCHEMA: &str = "stado.registry-push-receipt.v1";
const PULL_RECEIPT_SCHEMA: &str = "stado.registry-pull-receipt.v1";

fn print_push_receipt(receipt: &RegistryPushReceipt) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(receipt)?);
    Ok(())
}

/// Read the canonical document, apply a pure transform to it, and write the
/// result back conditionally on the generation that read produced — retrying
/// the whole round when somebody else wrote first.
///
/// This is the correct loop for exactly one shape of caller: one whose
/// `transform` is a function of the document and nothing else. Re-running such
/// a transform against a newer document is the whole point, because the answer
/// it produces is the answer for THAT document. A caller that has already
/// installed a key, stopped a unit or probed a host between its read and its
/// write must NOT be here: re-applying its transform would republish a
/// decision taken against state that has since changed. Those callers take a
/// single conditional attempt from their own [`fetch_versioned_document`] and
/// let the conflict reach the operator.
///
/// Only the conflict is retried. A validation refusal or a storage failure is
/// returned on the first round: neither becomes true by trying again.
pub async fn commit_document<F>(transform: F) -> Result<String, CmdError>
where
    F: Fn(&Value) -> Result<Value, CmdError>,
{
    for _ in 0..COMMIT_ROUNDS {
        let (document, expected_generation) = fetch_versioned_document().await?;
        let next = transform(&document)?;
        if next == document {
            return Ok(expected_generation);
        }
        match push_document_if(&next, &expected_generation).await {
            Ok(generation) => return Ok(generation),
            Err(error) if error.code == REGISTRY_CONFLICT_EXIT => continue,
            Err(error) => return Err(error),
        }
    }
    Err(RegistryConflict {
        location: targets::registry_location(),
        expected: format!("whatever {COMMIT_ROUNDS} consecutive reads returned"),
        actual: RegistryActual::Raced,
    }
    .error())
}

/// Validate a candidate against the document it would replace.
///
/// An `inference` fault that this write does not touch is returned rather than
/// raised: see [`crate::targets::validate_registry_for_write`]. Reading the
/// current document is best-effort, because a store that cannot be read is
/// reported by the write itself a moment later, and failing here would just
/// move the same error earlier with a less useful sentence.
async fn validate_for_write(document: &Value) -> Result<Option<String>, CmdError> {
    let current = match RegistryStore::open().await {
        Ok(store) => store
            .read_versioned()
            .await
            .ok()
            .flatten()
            .and_then(|blob| serde_json::from_str::<Value>(&blob.content).ok()),
        Err(_) => None,
    };
    crate::targets::validate_registry_for_write(document, current.as_ref())
        .map_err(|exc| CmdError::click(exc.to_string()))
}

/// Say out loud that a pre-existing fault was carried past, so a scoped write
/// never looks like a clean one.
fn warn_scoped_validation(pre_existing: Option<String>) {
    if let Some(detail) = pre_existing {
        eprintln!(
            "[registry] proceeding: this write leaves `inference` byte-identical, but that \
             section is already invalid and every write touching it will be refused until it \
             is repaired: {detail}"
        );
    }
}

/// Validate an in-memory document and compare-and-swap it against the
/// generation the caller read it at; returns the new generation.
///
/// The only programmatic write path. Validation runs BEFORE any store call,
/// so a document that would not validate never reaches the registry, and a
/// lost condition comes back as [`REGISTRY_CONFLICT_EXIT`] rather than a
/// generic failure — that is what lets [`commit_document`] retry a pure
/// transform and every other caller report the race instead of forcing.
pub async fn push_document_if(
    document: &Value,
    expected_generation: &str,
) -> Result<String, CmdError> {
    warn_scoped_validation(validate_for_write(document).await?);
    let payload = format!("{}\n", serde_json::to_string_pretty(document)?);
    let store = RegistryStore::open().await?;
    let generation = match store.compare_and_swap(expected_generation, &payload).await {
        Ok(generation) => generation,
        // Every backend reports both a moved generation and a missing object
        // this way, and neither tells this function which one it got, so the
        // sentence names what is certain: the document is not the one the
        // caller read.
        Err(StorageError::StorageConflict(_)) => {
            return Err(RegistryConflict {
                location: store.location().to_string(),
                expected: expected_generation.to_string(),
                actual: RegistryActual::Raced,
            }
            .error());
        }
        Err(error) => {
            return Err(CmdError::click(format!(
                "registry compare-and-swap failed: {error}"
            )));
        }
    };
    let confirmed = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click("registry compare-and-swap verification found no object"))?;
    if confirmed.version != generation || confirmed.content != payload {
        return Err(CmdError::click(
            "registry compare-and-swap verification returned different bytes",
        ));
    }
    Ok(generation)
}

/// The canonical document and the generation it was read at, which together
/// are the only safe input to [`push_document_if`]: a generation from a
/// second read belongs to a possibly different document.
pub async fn fetch_versioned_document() -> Result<(Value, String), CmdError> {
    let store = RegistryStore::open().await?;
    let blob = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click(format!("no registry document at {}", store.location())))?;
    let document: Value = serde_json::from_str(&blob.content)?;
    if !document.is_object() {
        return Err(CmdError::click(format!(
            "registry at {} is not an object",
            store.location()
        )));
    }
    Ok((document, blob.version))
}

/// The canonical registry as its raw document, off the same object
/// [`push_document_if`] compare-and-swaps.
///
/// Read-modify-write callers work on the raw document rather than on
/// [`Registry`] because an edit here is a surgical key change, and the raw
/// value is the shortest path to one. [`Registry`] is no longer lossy —
/// unmodelled top-level keys round-trip through `Registry::extra` and
/// `Registry::to_document` writes them back — so either route preserves the
/// document; this one simply does not re-serialize the parts it never
/// touched.
pub async fn fetch_document() -> Result<Value, CmdError> {
    let store = RegistryStore::open().await?;
    let text = store
        .read_text()
        .await?
        .ok_or_else(|| CmdError::click(format!("no registry document at {}", store.location())))?;
    let document: Value = serde_json::from_str(&text)?;
    if !document.is_object() {
        return Err(CmdError::click(format!(
            "registry at {} is not an object",
            store.location()
        )));
    }
    Ok(document)
}

/// `stado registry pull [--with-generation | --generation-only]` — print the
/// canonical registry.
///
/// Bare, it prints the pretty document and nothing else: scripts pipe this
/// into `jq`. `--with-generation` prints one
/// `stado.registry-pull-receipt.v1` object carrying the document and the
/// token `push --if-generation` spends, and `--generation-only` prints just
/// the token. Both come from ONE versioned read, because a generation read
/// separately from the document it is supposed to describe is a token for a
/// document nobody looked at.
pub async fn pull(with_generation: bool, generation_only: bool) -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let blob = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click(format!(
            "could not fetch registry from {}",
            store.location()
        ))
    })?;
    if generation_only {
        println!("{}", blob.version);
        return Ok(());
    }
    let value: Value = serde_json::from_str(&blob.content)?;
    if with_generation {
        println!(
            "{}",
            serde_json::to_string_pretty(&RegistryPullReceipt {
                schema: PULL_RECEIPT_SCHEMA.to_string(),
                location: store.location().to_string(),
                generation: blob.version,
                document: value,
            })?
        );
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// `stado registry self [--name-only]` — which registry target is this
/// machine. Installers need it: a plist that hardcodes a name the registry
/// does not carry produces a daemon that starts, fails its identity lookup
/// and exits, on every respawn, forever.
pub async fn self_target(name_only: bool) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = read_registry().await?;
    let found = registry
        .lookup_self(&hostname)
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "host {hostname} is not in {}",
                targets::registry_location()
            ))
        })?;
    if name_only {
        println!("{}", found.name);
    } else {
        println!("{}\t{}\t{}", found.name, found.kind, hostname);
    }
    Ok(())
}

/// `stado registry host add HOST --ssh DEST [--kind local]` — onboard a
/// machine into the canonical registry.
///
/// Refuses a name the registry already declares, and runs the exact
/// validation [`push`] runs BEFORE anything is written, so a colliding
/// hostname alias or an ssh destination with no host is rejected with the
/// registry-v2 contract's own message instead of landing in the store.
pub async fn host_add(
    host: &str,
    ssh: &str,
    kind: &str,
    release_platform: &str,
) -> Result<(), CmdError> {
    let name = targets::normalize_hostname(host);
    if name.is_empty() {
        return Err(CmdError::click("HOST must not be empty"));
    }
    if ssh.trim().is_empty() {
        return Err(CmdError::click("--ssh must not be empty"));
    }
    let location = targets::registry_location();
    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let entries = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let duplicate = entries.iter().find(|entry| {
        entry
            .get("name")
            .and_then(Value::as_str)
            .map(targets::normalize_hostname)
            .as_deref()
            == Some(name.as_str())
    });
    if let Some(entry) = duplicate {
        let declared_kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(CmdError::click(format!(
            "{name} is already declared in {location} (kind={declared_kind}); \
             refusing to add a duplicate"
        )));
    }
    entries.push(json!({
        "name": name,
        "kind": kind,
        "ssh": ssh,
        "release_platform": release_platform,
        "notes": "onboarded by `stado registry host add`",
    }));
    let generation = push_document_if(&document, &expected_generation).await?;
    println!(
        "added {name} (kind={kind}, release_platform={release_platform}, ssh={ssh}) -> \
         {location} generation={generation}"
    );
    Ok(())
}

fn registry_host_index(document: &Value, host: &str) -> Result<(usize, String), CmdError> {
    let name = targets::normalize_hostname(host);
    if name.is_empty() {
        return Err(CmdError::click("HOST must not be empty"));
    }
    let entries = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let index = entries
        .iter()
        .position(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .map(targets::normalize_hostname)
                .as_deref()
                == Some(name.as_str())
        })
        .ok_or_else(|| CmdError::click(format!("registry target {name:?} not found")))?;
    Ok((index, name))
}

/// List the preferred host connection followed by every ordered fallback.
pub async fn host_path_list(host: &str, json_output: bool) -> Result<(), CmdError> {
    let (document, _) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entry = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|entries| entries.get(index))
        .cloned()
        .ok_or_else(|| CmdError::click("registry target disappeared"))?;
    let target: ComputeTarget = serde_json::from_value(entry)?;
    let connections = target
        .ssh_connections()
        .enumerate()
        .map(|(order, (path, destination))| {
            json!({
                "name": path,
                "destination": destination,
                "order": order,
                "preferred": order == 0,
            })
        })
        .collect::<Vec<_>>();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": name,
                "connections": connections,
            }))?
        );
    } else if connections.is_empty() {
        println!("{name}: no SSH connection paths");
    } else {
        for connection in connections {
            let path = connection["name"].as_str().unwrap_or_default();
            let destination = connection["destination"].as_str().unwrap_or_default();
            let role = if connection["preferred"].as_bool().unwrap_or(false) {
                "preferred"
            } else {
                "fallback"
            };
            println!("{name}\t{path}\t{role}\t{destination}");
        }
    }
    Ok(())
}

/// One machine-readable answer for every `path set` outcome, including the
/// idempotent one. Desktop clients must not scrape the human sentence to learn
/// whether the registry moved.
fn print_host_path_set_receipt(
    target: &str,
    path: &str,
    destination: &str,
    generation: &str,
    changed: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "path": path,
                "destination": destination,
                "changed": changed,
                "generation": generation,
            }))?
        );
    } else if changed {
        println!("set {target} connection path {path} -> {destination}; generation={generation}");
    } else if path == targets::PRIMARY_SSH_CONNECTION {
        println!("{target}: primary already points to {destination}");
    } else {
        println!("{target}: {path} already points to {destination}");
    }
    Ok(())
}

/// The remove receipt distinguishes an idempotent absence from a registry
/// write while preserving the existing terminal sentence.
fn print_host_path_remove_receipt(
    target: &str,
    path: &str,
    generation: &str,
    removed: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "path": path,
                "removed": removed,
                "generation": generation,
            }))?
        );
    } else if removed {
        println!("removed {target} connection path {path}; generation={generation}");
    } else {
        println!("{target}: connection path {path} is already absent");
    }
    Ok(())
}

/// Add or replace one host connection path in the canonical registry.
pub async fn host_path_set(
    host: &str,
    path: &str,
    ssh: &str,
    priority: Option<usize>,
    json_output: bool,
) -> Result<(), CmdError> {
    let path = path.trim();
    let destination = ssh.trim();
    if path.is_empty() {
        return Err(CmdError::click("PATH must not be empty"));
    }
    if destination.is_empty() {
        return Err(CmdError::click("--ssh must not be empty"));
    }
    if priority == Some(0) {
        return Err(CmdError::click("--priority starts at 1"));
    }
    if path == targets::PRIMARY_SSH_CONNECTION && priority.is_some() {
        return Err(CmdError::click(
            "the primary path is always preferred and does not take --priority",
        ));
    }

    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|entries| entries.get_mut(index))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;

    if path == targets::PRIMARY_SSH_CONNECTION {
        if entry.get("ssh").and_then(Value::as_str) == Some(destination) {
            return print_host_path_set_receipt(
                &name,
                path,
                destination,
                &expected_generation,
                false,
                json_output,
            );
        }
        entry.insert("ssh".to_string(), json!(destination));
    } else {
        let paths = entry
            .entry("ssh_fallbacks".to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| CmdError::click("target.ssh_fallbacks must be an array"))?;
        let existing = paths
            .iter()
            .position(|candidate| candidate.get("name").and_then(Value::as_str) == Some(path));
        let candidate = json!({"name": path, "destination": destination});
        if priority.is_none()
            && existing.is_some_and(|position| paths.get(position) == Some(&candidate))
        {
            return print_host_path_set_receipt(
                &name,
                path,
                destination,
                &expected_generation,
                false,
                json_output,
            );
        }
        let default_position = existing.unwrap_or(paths.len());
        if let Some(position) = existing {
            paths.remove(position);
        }
        let insertion = match priority {
            Some(value) if value > paths.len() + 1 => {
                return Err(CmdError::click(format!(
                    "--priority {value} is outside 1..={}",
                    paths.len() + 1
                )))
            }
            Some(value) => value - 1,
            None => default_position.min(paths.len()),
        };
        paths.insert(insertion, candidate);
    }

    let generation = push_document_if(&document, &expected_generation).await?;
    print_host_path_set_receipt(&name, path, destination, &generation, true, json_output)
}

/// Remove one fallback path; the preferred path is replaced through `set`.
pub async fn host_path_remove(host: &str, path: &str, json_output: bool) -> Result<(), CmdError> {
    let path = path.trim();
    if path == targets::PRIMARY_SSH_CONNECTION {
        return Err(CmdError::click(
            "the primary path cannot be removed; replace it with `registry host path set`",
        ));
    }
    if path.is_empty() {
        return Err(CmdError::click("PATH must not be empty"));
    }

    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let (index, name) = registry_host_index(&document, host)?;
    let entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|entries| entries.get_mut(index))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;
    let Some(paths) = entry.get_mut("ssh_fallbacks").and_then(Value::as_array_mut) else {
        return print_host_path_remove_receipt(
            &name,
            path,
            &expected_generation,
            false,
            json_output,
        );
    };
    let Some(position) = paths
        .iter()
        .position(|candidate| candidate.get("name").and_then(Value::as_str) == Some(path))
    else {
        return print_host_path_remove_receipt(
            &name,
            path,
            &expected_generation,
            false,
            json_output,
        );
    };
    paths.remove(position);
    if paths.is_empty() {
        entry.remove("ssh_fallbacks");
    }

    let generation = push_document_if(&document, &expected_generation).await?;
    print_host_path_remove_receipt(&name, path, &generation, true, json_output)
}

// ---------------------------------------------------------------------------
// live host state: beacons + capacity broadcasts
// ---------------------------------------------------------------------------

/// A beacon older than the fleet's liveness window is a divergence, not
/// jitter: the beacon republishes on the same cadence as the capacity
/// broadcast (`constants::CAPACITY_HEARTBEAT_INTERVAL_S` seconds — the
/// LaunchAgent `StartInterval` rendered by
/// `deploy/install_macos_coordinator.sh`, and the systemd unit in
/// `deploy/host-health-beacon.service`), so
/// [`capacity::CAPACITY_STALE_SECONDS`] is the same missed-publications
/// window `queue::capacity` already applies to the other liveness signal.
/// One window, both signals.
fn stale_after_seconds() -> i64 {
    capacity::CAPACITY_STALE_SECONDS as i64
}

/// One `host_health/<slug>.json` object.
struct Beacon {
    /// Store-relative object name.
    path: String,
    /// Object mtime: the authority for age, since the body's `reported_at`
    /// is stamped by the reporting host's own clock.
    updated: Option<DateTime<Utc>>,
    /// Parsed beacon body; `None` when the object is unparsable or is not
    /// a JSON object.
    body: Option<Map<String, Value>>,
}

impl Beacon {
    /// `reported_at` as the host stamped it.
    fn reported_at(&self) -> Option<&str> {
        self.body.as_ref()?.get("reported_at")?.as_str()
    }

    /// When this beacon was last known good: the object mtime, falling
    /// back to the host's own `reported_at` for backends that carry no
    /// mtime.
    fn observed_at(&self) -> Option<DateTime<Utc>> {
        self.updated.or_else(|| {
            DateTime::parse_from_rfc3339(self.reported_at()?)
                .ok()
                .map(|ts| ts.with_timezone(&Utc))
        })
    }

    /// State of one unit in the beacon's `units` map. Values are either
    /// `{"state": ...}` objects or a bare string, exactly as
    /// `monitor::host_health::format_host_health` reads them.
    fn unit_state(&self, unit: &str) -> Option<String> {
        let value = self.body.as_ref()?.get("units")?.as_object()?.get(unit)?;
        match value {
            Value::Object(state) => Some(
                state
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            Value::String(state) => Some(state.clone()),
            other => Some(other.to_string()),
        }
    }

    /// Whether a `scheduled` unit carries the complete native evidence the
    /// publisher requires before assigning that state.
    fn scheduled_unit_is_healthy(&self, unit: &str) -> bool {
        let Some(fields) = self
            .body
            .as_ref()
            .and_then(|body| body.get("units"))
            .and_then(Value::as_object)
            .and_then(|units| units.get(unit))
            .and_then(Value::as_object)
        else {
            return false;
        };
        let common_evidence = fields.get("state").and_then(Value::as_str) == Some(SCHEDULED_STATE)
            && fields.get("service_type").and_then(Value::as_str) == Some("oneshot")
            && fields
                .get("manager")
                .and_then(Value::as_str)
                .is_some_and(|manager| matches!(manager, "system" | "user"))
            && fields
                .get("triggered_by")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    fields
                        .get("active_trigger")
                        .and_then(Value::as_str)
                        .is_some_and(|active| {
                            triggers
                                .iter()
                                .any(|trigger| trigger.as_str() == Some(active))
                        })
                })
            && fields
                .get("trigger_state")
                .and_then(Value::as_str)
                .is_some_and(|state| matches!(state, "active" | "activating" | "reloading"));
        if !common_evidence {
            return false;
        }
        match fields.get("run_state").and_then(Value::as_str) {
            Some("running") => {
                fields.get("native_state").and_then(Value::as_str) == Some("activating")
            }
            Some("succeeded") => {
                fields
                    .get("native_state")
                    .and_then(Value::as_str)
                    .is_some_and(|state| matches!(state, "inactive" | "active" | "reloading"))
                    && fields.get("result").and_then(Value::as_str) == Some("success")
                    && fields.get("exec_main_status").and_then(Value::as_str) == Some("0")
                    && fields
                        .get("last_started_at")
                        .and_then(Value::as_str)
                        .is_some_and(|stamp| !stamp.is_empty() && stamp != "n/a")
            }
            _ => false,
        }
    }
}

/// Every beacon in the store, keyed by slug — the `<slug>.json` stem that
/// `monitor::host_health::beacon_slugs` resolves targets to.
async fn load_beacons(store: &JobStorage) -> Result<BTreeMap<String, Beacon>, CmdError> {
    let prefix = format!("{}/", host_health::HEALTH_PREFIX);
    let mut beacons = BTreeMap::new();
    for blob in store.list_blobs_with_meta(&prefix).await? {
        let Some(slug) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|stem| stem.strip_suffix(".json"))
        else {
            continue;
        };
        let body = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            });
        beacons.insert(
            slug.to_string(),
            Beacon {
                path: blob.name.clone(),
                updated: blob.updated,
                body,
            },
        );
    }
    Ok(beacons)
}

/// The newest beacon a target resolves to, by the same slug rule
/// `monitor::host_health::load_host_health` resolves forward.
fn beacon_for_slugs<'a>(
    slugs: &[String],
    beacons: &'a BTreeMap<String, Beacon>,
) -> Option<&'a Beacon> {
    let mut selected: Option<&Beacon> = None;
    for slug in slugs {
        let Some(candidate) = beacons.get(slug) else {
            continue;
        };
        if selected.is_none_or(|current| candidate.observed_at() > current.observed_at()) {
            selected = Some(candidate);
        }
    }
    selected
}

fn beacon_for<'a>(
    target: &ComputeTarget,
    beacons: &'a BTreeMap<String, Beacon>,
) -> Option<&'a Beacon> {
    let slugs = host_health::beacon_slugs(target, &target.name);
    beacon_for_slugs(&slugs, beacons)
}

/// One service a registry target declares it manages.
struct DeclaredUnit {
    /// Operator-facing service name.
    name: String,
    /// The identifier the beacon reports it under: the launchd label on
    /// macOS, the systemd unit on Linux.
    id: String,
}

/// Services declared on a target by `stado service adopt|deploy`
/// (`deploy/service.rs`), which writes a per-target `services` array. The
/// key is unknown to [`ComputeTarget`], so it round-trips through
/// [`ComputeTarget::extra`]; a target that declares none is checked for
/// liveness only, never for units.
fn declared_units(
    target: &ComputeTarget,
    release_control: Option<&crate::release_control::ReleaseControl>,
) -> Vec<DeclaredUnit> {
    let legacy_labels = service::legacy_launchd_labels(target, release_control);
    let Some(services) = target.extra.get("services").and_then(Value::as_array) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
            };
            let id = text("label")
                .or_else(|| text("unit"))
                .or_else(|| text("name"))?;
            if legacy_labels.contains(id) {
                return None;
            }
            Some(DeclaredUnit {
                name: text("name").unwrap_or(id).to_string(),
                id: id.to_string(),
            })
        })
        .collect()
}

/// The canonical registry for a read-only command, the last-known-good copy
/// when the authority does not answer, or the reason neither could be READ.
///
/// Never an empty registry: `doctor` reporting "every host is unmanaged"
/// because the store was down is the exact confusion
/// `targets::RegistryFetchError` exists to prevent. Never a silent copy
/// either — the copy's age goes to stderr in one sentence, because a
/// diagnostic that dies with the thing it diagnoses is worthless, and a
/// diagnostic that answers from a copy without saying so is worse.
pub(crate) async fn read_registry() -> Result<Registry, CmdError> {
    let (registry, notice) = targets::fetch_registry_or_last_good()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if let Some(notice) = notice {
        targets::report_registry_notice(&notice);
    }
    Ok(registry)
}

// ---------------------------------------------------------------------------
// registry doctor
// ---------------------------------------------------------------------------

/// One way the registry and the live fleet disagree.
struct Finding {
    /// Stable machine-readable category.
    kind: &'static str,
    /// Target, host slug or consumer the finding is about.
    subject: String,
    /// What specifically disagrees.
    detail: String,
    /// Unit label this finding is about, when it is about one.
    ///
    /// Two rows for one cause is noise. A unit that is missing because its host
    /// cannot satisfy what the unit needs produces both a `capability-unsatisfied`
    /// and a `missing-plist`; the second is the symptom of the first, and
    /// [`doctor`] drops it so the row that survives names the cause.
    unit: Option<String>,
}

impl Finding {
    fn new(kind: &'static str, subject: impl AsRef<str>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            subject: subject.as_ref().to_string(),
            detail: detail.into(),
            unit: None,
        }
    }

    /// Name the unit this finding is about, so one cause cannot be reported twice.
    fn about(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    fn to_json(&self) -> Value {
        json!({"finding": self.kind, "subject": self.subject, "detail": self.detail})
    }
}

/// Object prefix the per-host capability measurements live under, alongside
/// `host_health/` and read through the same object API
/// (`stado://<namespace>/host_capabilities/<registry-target-name>.json`).
const CAPABILITIES_PREFIX: &str = "host_capabilities";

/// The only measurement schema this build understands. A document carrying
/// anything else is reported rather than guessed at: a requirement checked
/// against a shape nobody agreed on is worse than an unchecked one.
const CAPABILITIES_SCHEMA: &str = "wisent.host-capabilities.v1";

/// One measured capability: the answer, and the measurement that produced it.
pub(crate) struct MeasuredCapability {
    pub(crate) value: bool,
    pub(crate) detail: String,
}

/// One `host_capabilities/<target>.json` object.
pub(crate) struct Measurement {
    /// Store-relative object name, so a finding names what to go and read.
    path: String,
    schema: String,
    /// The host's own stamp, falling back to the object mtime the same way
    /// [`Beacon::observed_at`] does.
    pub(crate) measured_at: Option<DateTime<Utc>>,
    pub(crate) capabilities: BTreeMap<String, MeasuredCapability>,
}

/// Every capability measurement in the store, keyed by the registry target name
/// the object is published under.
///
/// One prefix listing plus the bodies it finds, exactly like [`load_beacons`]:
/// the two signals are published the same way and are read the same way.
pub(crate) async fn load_capability_measurements(
    store: &JobStorage,
) -> Result<BTreeMap<String, Measurement>, crate::queue::StorageError> {
    let prefix = format!("{CAPABILITIES_PREFIX}/");
    let mut measurements = BTreeMap::new();
    for blob in store.list_blobs_with_meta(&prefix).await? {
        let Some(target) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|stem| stem.strip_suffix(".json"))
        else {
            continue;
        };
        let Some(body) = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let capabilities = body
            .get("capabilities")
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(id, entry)| {
                        (
                            id.clone(),
                            MeasuredCapability {
                                value: entry.get("value").and_then(Value::as_bool) == Some(true),
                                detail: entry
                                    .get("detail")
                                    .and_then(Value::as_str)
                                    .unwrap_or("no detail recorded")
                                    .to_string(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        measurements.insert(
            target.to_string(),
            Measurement {
                path: blob.name.clone(),
                schema: body
                    .get("schema")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                measured_at: body
                    .get("measured_at")
                    .and_then(Value::as_str)
                    .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                    .map(|stamp| stamp.with_timezone(&Utc))
                    .or(blob.updated),
                capabilities,
            },
        );
    }
    Ok(measurements)
}

/// Object prefix the published job requirement declarations live under, read
/// through the same object API as the beacons and the measurements.
const REQUIREMENTS_PREFIX: &str = "job_requirements";

/// The only requirement schema this build understands.
const REQUIREMENTS_SCHEMA: &str = "wisent.trajectory-requirements.v1";

/// How long a published requirement declaration is believed.
///
/// Deliberately not the beacon window: a requirement is republished when the job
/// changes, not on a heartbeat, so judging it in minutes would mark every
/// declaration stale within one and teach operators to skip the row. A week is
/// longer than the fleet goes between Weles releases, so an object older than
/// that is a declaration nobody is republishing — which is the failure this
/// window exists to catch, an object that no longer matches the repository file
/// it is supposed to be a copy of.
fn requirements_stale_after_seconds() -> i64 {
    TimeDelta::weeks(1).num_seconds()
}

/// One declared service and the job it runs.
struct RequirementClaim {
    /// Service name as the registry spells it.
    unit: String,
    /// The identifier the beacon reports the unit under, so a `missing-plist` row
    /// for the same unit can be recognised as this finding's symptom.
    label: String,
    /// Trajectory id the service entry names, e.g. `kimi/login`.
    trajectory: String,
}

/// Every published requirement declaration, resolved to what each trajectory
/// needs.
struct Declarations {
    /// Trajectory id -> the object that declares it and the capability ids it
    /// names.
    needs: BTreeMap<String, (String, Vec<String>)>,
    /// Objects found under the prefix, so a finding can say what was consulted.
    objects: Vec<String>,
    /// Objects this build would not read, and why. A service pointing into one is
    /// reported rather than silently treated as satisfied.
    refused: Vec<String>,
}

impl Declarations {
    /// What was consulted, for a finding that has to explain an absence.
    fn consulted(&self) -> String {
        let read = if self.objects.is_empty() {
            format!("no object exists under {REQUIREMENTS_PREFIX}/")
        } else {
            format!("read {}", self.objects.join(", "))
        };
        if self.refused.is_empty() {
            read
        } else {
            format!("{read}; refused {}", self.refused.join("; "))
        }
    }
}

/// Every requirement declaration in the store.
///
/// The declaration is the job author's document, published verbatim
/// (`stado://<namespace>/job_requirements/weles-trajectories.json` carries the
/// bytes of `weles/scripts/trajectories/requirements.json`). The registry names
/// which job a service runs and nothing more, so this is the one place a
/// capability list for a job exists — a copy in the registry would be the second
/// source of truth that `unread-declaration` and this whole command exist to
/// prevent.
async fn load_job_requirements(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<Declarations, crate::queue::StorageError> {
    let prefix = format!("{REQUIREMENTS_PREFIX}/");
    let mut declarations = Declarations {
        needs: BTreeMap::new(),
        objects: Vec::new(),
        refused: Vec::new(),
    };
    for blob in store.list_blobs_with_meta(&prefix).await? {
        if !blob.name.ends_with(".json") {
            continue;
        }
        declarations.objects.push(blob.name.clone());
        let Some(body) = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            declarations
                .refused
                .push(format!("{} is not readable JSON", blob.name));
            continue;
        };
        let schema = body
            .get("schema")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if schema != REQUIREMENTS_SCHEMA {
            declarations.refused.push(format!(
                "{} carries schema {schema:?} rather than {REQUIREMENTS_SCHEMA}",
                blob.name
            ));
            continue;
        }
        if let Some(published) = blob.updated {
            let age = now - published;
            if age.num_seconds() > requirements_stale_after_seconds() {
                declarations.refused.push(format!(
                    "{} was published {} ago ({}), past the {}s republication window",
                    blob.name,
                    human_age(age),
                    published.to_rfc3339(),
                    requirements_stale_after_seconds()
                ));
                continue;
            }
        }
        let Some(trajectories) = body.get("trajectories").and_then(Value::as_object) else {
            declarations
                .refused
                .push(format!("{} carries no trajectories map", blob.name));
            continue;
        };
        for (trajectory, value) in trajectories {
            // A non-string entry is dropped rather than guessed at: a garbled
            // requirement must not be able to pass as a satisfied one.
            let capabilities = value
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            declarations
                .needs
                .insert(trajectory.clone(), (blob.name.clone(), capabilities));
        }
    }
    Ok(declarations)
}

/// Which job each service declared on this target runs.
///
/// The registry's whole role in this join is the identifier: `trajectory` on the
/// service entry, never a capability list. A service that names no trajectory
/// declares no requirement, so every registry written before this field existed
/// stays clean.
fn declared_trajectories(target: &ComputeTarget) -> Vec<RequirementClaim> {
    let Some(services) = target.extra.get("services").and_then(Value::as_array) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|entry| {
            let trajectory = entry
                .get("trajectory")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            };
            let unit = text("name").or_else(|| text("label"));
            Some(RequirementClaim {
                unit: unit.unwrap_or("(unnamed service)").to_string(),
                // The beacon reports a unit under its launchd label or systemd
                // unit, and that is the key the missing-plist row is filed
                // under, so it is resolved here exactly as `declared_units`
                // resolves it.
                label: text("label")
                    .or_else(|| text("unit"))
                    .or(unit)
                    .unwrap_or_default()
                    .to_string(),
                trajectory: trajectory.to_string(),
            })
        })
        .collect()
}

/// What stops one host's last measurement from satisfying a capability list, one
/// clause per reason, empty when nothing does.
///
/// The single place that answers "can this host run this job", so the finding
/// about a host that declares the job and the finding about a job no host
/// declares cannot answer it differently. A missing, mis-schema'd or stale
/// measurement disqualifies the host in one clause rather than once per
/// capability: the operator's next action is the same however many ids were
/// named, and it is to go and measure the host.
fn measurement_gaps(
    target: &str,
    capabilities: &[String],
    measurement: Option<&Measurement>,
    now: DateTime<Utc>,
) -> Vec<String> {
    // A job that needs nothing of the host is satisfied by every host, measured
    // or not: `codex/reauth` declares exactly that.
    if capabilities.is_empty() {
        return Vec::new();
    }
    let Some(measurement) = measurement else {
        return vec![format!(
            "{CAPABILITIES_PREFIX}/{target}.json does not exist: nothing has measured this host"
        )];
    };
    if measurement.schema != CAPABILITIES_SCHEMA {
        return vec![format!(
            "{} carries schema {:?} rather than {CAPABILITIES_SCHEMA}",
            measurement.path, measurement.schema
        )];
    }
    match measurement.measured_at {
        None => {
            return vec![format!(
                "{} carries neither measured_at nor an object timestamp, so its age cannot be \
                 judged",
                measurement.path
            )]
        }
        Some(measured) => {
            let age = now - measured;
            if age.num_seconds() > stale_after_seconds() {
                return vec![format!(
                    "{} was measured {} ago ({}), past the {}s liveness window",
                    measurement.path,
                    human_age(age),
                    measured.to_rfc3339(),
                    stale_after_seconds()
                )];
            }
        }
    }
    capabilities
        .iter()
        .filter_map(
            |capability| match measurement.capabilities.get(capability) {
                None => Some(format!(
                    "{} does not measure {capability}",
                    measurement.path
                )),
                Some(measured) if !measured.value => {
                    Some(format!("{capability} measured false: {}", measured.detail))
                }
                Some(_) => None,
            },
        )
        .collect()
}

/// Every hop of the join that fails for one declared service, one sentence each:
/// declared service -> trajectory id -> published requirement -> measured
/// capability. Empty means the host is measured able to run what it is declared to
/// run.
fn unmet_requirements(
    target: &str,
    claim: &RequirementClaim,
    declarations: &Declarations,
    measurement: Option<&Measurement>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let unit = &claim.unit;
    let trajectory = &claim.trajectory;
    let Some((source, capabilities)) = declarations.needs.get(trajectory) else {
        return vec![format!(
            "{unit} runs trajectory {trajectory}, and no published declaration names it: {}",
            declarations.consulted()
        )];
    };
    let needs = capabilities.join(", ");
    measurement_gaps(target, capabilities, measurement, now)
        .into_iter()
        .map(|gap| {
            format!("{unit} runs {trajectory}, which {source} says requires {needs}, and {gap}")
        })
        .collect()
}

/// Jobs in the published roster that nothing runs and no host could.
///
/// A declared service entry is the RESULT of a placement that succeeded, so a
/// trajectory no target declares is a job waiting for a host. That is only worth
/// reporting when no candidate can take it: while some measured host satisfies the
/// requirement, placement has an answer and the absence is a step not yet taken
/// rather than a contradiction. The row names each candidate and the measurement
/// that disqualified it, so it says what would have to change, and it disappears by
/// itself the moment a capable host exists and the unit is adopted where it runs.
fn unplaced_jobs(
    registry: &Registry,
    declarations: &Declarations,
    measurements: &BTreeMap<String, Measurement>,
    placed: &BTreeSet<&str>,
    now: DateTime<Utc>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (trajectory, (source, capabilities)) in &declarations.needs {
        if capabilities.is_empty() || placed.contains(trajectory.as_str()) {
            continue;
        }
        let mut disqualified = Vec::new();
        for target in &registry.targets {
            // Only kind=local names a machine that can hold a session and run a
            // browser; "gcp" and "vast" targets are dispatcher pools.
            if !target.is_provider(crate::capabilities::ProviderId::Local) {
                continue;
            }
            let gaps = measurement_gaps(
                &target.name,
                capabilities,
                measurements.get(&target.name),
                now,
            );
            if gaps.is_empty() {
                disqualified.clear();
                break;
            }
            disqualified.push(format!("{}: {}", target.name, gaps.join(", ")));
        }
        if !disqualified.is_empty() {
            findings.push(Finding::new(
                "job-unplaced",
                trajectory,
                format!(
                    "{source} says it requires {}, no registry target declares a service that \
                     runs it, and no host can take it — {}",
                    capabilities.join(", "),
                    disqualified.join("; ")
                ),
            ));
        }
    }
    findings
}

/// The value at a dotted path, or `None` when any segment is absent.
fn value_at<'a>(root: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(root, |value, key| value.get(key))
}

/// One line of JSON for a declared value, so a finding shows what was written
/// rather than only where.
fn rendered(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// Why a declared value never reaches behaviour, or `None` when it does.
///
/// The catalog decides, in one place, for both surfaces: a fleet reader whose
/// reachability condition holds is read, and everything else is a declaration an
/// operator wrote for nobody.
fn unread_reason(field: &DeclaredField, sibling: Option<String>) -> Option<String> {
    match field.consumer {
        Consumer::Fleet(reader) => {
            let condition = field.reached_when?;
            let observed = sibling.unwrap_or_else(|| "(absent)".to_string());
            if observed.starts_with(condition.value_prefix) {
                return None;
            }
            Some(format!(
                "its only reader {reader} runs when {} starts with {:?}, and that key is {observed}",
                condition.path, condition.value_prefix
            ))
        }
        Consumer::Unread => Some("no code path in this build reads it".to_string()),
    }
}

/// A sibling value rendered for a finding: strings bare, everything else as JSON,
/// so a URL reads as a URL and a number still reads unambiguously.
fn sibling_value(root: &Value, condition: SiblingCondition) -> Option<String> {
    value_at(root, condition.path).map(|value| {
        value
            .as_str()
            .map_or_else(|| rendered(value), str::to_string)
    })
}

/// Registry fields on one target that no consumer reads, from both halves of the
/// rule.
///
/// The derived half needs no catalog: a key [`ComputeTarget`] does not model
/// lands in `extra`, which is by construction the set of keys no typed reader in
/// this build can name, so a field added tomorrow with no reader fails without
/// anybody remembering to declare it. The catalogued half covers what the derived
/// half cannot see — a path inside a block this build does model, where the
/// deserializer accepting a value proves nothing about anyone acting on it.
fn unread_declarations(target: &ComputeTarget, entry: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (key, value) in &target.extra {
        match crate::capabilities::declared_field(DeclarationSurface::RegistryTarget, key) {
            Some(field) => {
                if let Some(reason) = unread_reason(
                    field,
                    field.reached_when.and_then(|c| sibling_value(entry, c)),
                ) {
                    findings.push(Finding::new(
                        "unread-declaration",
                        &target.name,
                        format!(
                            "{} {key} is declared as {} but {reason}",
                            field.surface.label(),
                            rendered(value)
                        ),
                    ));
                }
            }
            None => findings.push(Finding::new(
                "unread-declaration",
                &target.name,
                format!(
                    "registry target key {key} is declared as {} and is neither modelled by \
                     ComputeTarget nor catalogued in capabilities::DECLARED_FIELDS, so no reader \
                     in this build can consult it",
                    rendered(value)
                ),
            )),
        }
    }
    // Dotted paths only: a top-level catalogued key that this build does not
    // model was already answered by the loop above, and answering it twice would
    // report one defect as two.
    for field in crate::capabilities::DECLARED_FIELDS {
        if field.surface != DeclarationSurface::RegistryTarget || !field.path.contains('.') {
            continue;
        }
        let Some(value) = value_at(entry, field.path) else {
            continue;
        };
        let Some(reason) = unread_reason(
            field,
            field.reached_when.and_then(|c| sibling_value(entry, c)),
        ) else {
            continue;
        };
        findings.push(Finding::new(
            "unread-declaration",
            &target.name,
            format!(
                "{} {} is declared as {} but {reason}",
                field.surface.label(),
                field.path,
                rendered(value)
            ),
        ));
    }
    findings
}

/// Configuration keys this deployment carries that no reader on it can consult.
///
/// The document is the one this process would honour, so the answer is about the
/// deployment actually running rather than about the schema. Reading it cannot
/// fail here: every caller reaches `doctor` through a storage handle that already
/// loaded and parsed the same file.
fn unread_configuration() -> Vec<Finding> {
    let subject = crate::config_file::config_path()
        .ok()
        .flatten()
        .map_or_else(
            || "stado config".to_string(),
            |path| path.display().to_string(),
        );
    let mut findings = Vec::new();
    for field in crate::capabilities::DECLARED_FIELDS {
        if field.surface != DeclarationSurface::Configuration {
            continue;
        }
        let Some(value) = crate::config_file::get(field.path).filter(|value| !value.is_null())
        else {
            continue;
        };
        let sibling = field
            .reached_when
            .and_then(|condition| crate::config_file::get(condition.path))
            .map(|value| {
                value
                    .as_str()
                    .map_or_else(|| rendered(&value), str::to_string)
            });
        if let Some(reason) = unread_reason(field, sibling) {
            findings.push(Finding::new(
                "unread-declaration",
                &subject,
                format!(
                    "{} {} is declared as {} but {reason}",
                    field.surface.label(),
                    field.path,
                    rendered(&value)
                ),
            ));
        }
    }
    findings
}

/// `stado registry doctor [--json]` — diff registry declarations against
/// live host state: hosts with no heartbeat, stale beacons, missing
/// plists, unmanaged agents, and a build that refuses the document itself.
///
/// Exits non-zero on any divergence, naming each one, so it drops straight
/// into a cron or a CI gate. Liveness comes from the beacons and the
/// capacity broadcasts, never ssh: the whole command is one prefix listing
/// plus the bodies it finds.
#[allow(clippy::too_many_lines)]
pub async fn doctor(as_json: bool) -> Result<(), CmdError> {
    // Doctor needs the typed registry and raw extension blocks to describe one
    // observation. Derive both from one authoritative versioned read: using
    // `read_registry` here could select the last-known-good copy, and fetching
    // the raw document afterwards could then mix that copy with a different
    // authority generation.
    let (document, _) = fetch_versioned_document().await?;
    let registry = targets::load_registry_from_value(&document).map_err(|error| {
        CmdError::click(format!(
            "invalid registry document at {}: {error}",
            targets::registry_location()
        ))
    })?;
    let store = JobStorage::for_primary_reads().await?;
    let beacons = load_beacons(&store).await?;
    let consumers = capacity::read_consumer_capacity(&store).await?;
    let now = Utc::now();

    let mut findings: Vec<Finding> = Vec::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();

    // A document the inference contract refuses is not a cosmetic fault: every
    // resolver on the fleet validates the same way before it adopts a
    // generation, so it keeps serving the last copy it accepted and hands
    // consumers an address the fleet has since moved away from. On 2026-09-06 a
    // route alias without a namespace ("wisent-backend") published that state:
    // the always-on host's resolver froze eleven generations back, every chat
    // took `connection refused` from a candidate port nothing served any more,
    // and the only trace was one line in that resolver's log.
    if let Err(error) = crate::inference::schema::validate(&document) {
        findings.push(Finding::new(
            "resolver-refuses-registry",
            "registry",
            format!(
                "every resolver refuses this document and keeps serving the last \
                 generation it accepted, so consumers resolve to addresses this \
                 registry no longer declares: {error}"
            ),
        ));
    }

    // What the fleet declares DELIVERED, which is what a missing version
    // declaration is measured against. Read from the document's own
    // `release_control` block and never from a unit on the host: a product
    // stays a release target after its launchd plist is removed, and so does
    // the version gap. A block that will not parse is reported rather than
    // skipped — a check that quietly measures nothing is the defect it was
    // written to catch.
    let release_control = match registry
        .extra
        .get(crate::release_control::RELEASE_CONTROL_KEY)
    {
        Some(value) => {
            match <crate::release_control::ReleaseControl as serde::Deserialize>::deserialize(value)
            {
                Ok(control) => Some(control),
                Err(error) => {
                    findings.push(Finding::new(
                        "unreadable-release-control",
                        "registry",
                        format!(
                            "registry.{} did not parse, so no delivered product was judged \
                             against its declared version: {error}",
                            crate::release_control::RELEASE_CONTROL_KEY
                        ),
                    ));
                    None
                }
            }
        }
        None => None,
    };

    // A blue-green product serves consumers on its stable bind; its candidate
    // ports belong to the rollout and change with every version. A service
    // directory that hands consumers a candidate port therefore works only
    // until the next release: on 2026-09-06 the directory named brama's
    // 127.0.0.1:18080 while the policy declared 127.0.0.1:8080, and three
    // rollouts in one evening each took product chat down the moment the
    // rollout moved to the other candidate port.
    if let Some(control) = release_control.as_ref() {
        for (product, policy) in &control.products {
            for (host, target) in &policy.targets {
                let Ok(serving) = target.blue_green_serving() else {
                    continue;
                };
                let Some(route) = registry
                    .service_directory
                    .as_ref()
                    .and_then(|directory| directory.services.get(&policy.service))
                else {
                    continue;
                };
                let Some(endpoint) = route.endpoints.get(host) else {
                    continue;
                };
                let declared_port = serving
                    .stable_bind
                    .rsplit_once(':')
                    .and_then(|(_, port)| port.parse::<u16>().ok());
                let named_port = endpoint
                    .url
                    .rsplit_once(':')
                    .and_then(|(_, port)| port.trim_end_matches('/').parse::<u16>().ok());
                let Some(named) = named_port else {
                    continue;
                };
                if serving.candidate_ports.contains(&named) || declared_port != Some(named) {
                    findings.push(Finding::new(
                        "directory-names-candidate-port",
                        host,
                        format!(
                            "service directory sends {} consumers to {} while release-controlled \
                             product {product} declares its stable bind {}; the next rollout moves \
                             that port and every consumer loses the service",
                            policy.service, endpoint.url, serving.stable_bind
                        ),
                    ));
                }
            }
        }
    }

    // The one host whose unit files this command may open. Everything else
    // here is read from the store, and the environment a unit actually
    // carries is in no object in it: the beacon publishes one `state` word
    // per unit and the registry's own service record has no environment
    // field at all, so this is the only host where the declaration can be
    // confronted with the file. A registry that names no target for this
    // machine leaves it `None`, and every unit is then reported unread
    // rather than empty.
    let local_host = registry
        .lookup_self(&crate::providers::vast::system_hostname())
        .ok()
        .flatten()
        .map(|target| target.name.clone());

    // The raw document from the same versioned read that produced `registry`
    // above. `service_directory`, `service_resolver`, and release/build checks
    // need raw extension blocks, and a second fetch could answer with a
    // different generation. Doctor is an observation of the canonical
    // authority, so unlike general host-resolution commands it does not fall
    // back to the on-disk last-known-good copy. A refused primary read remains
    // a refusal instead of becoming findings about a stale document.

    for target in &registry.targets {
        let slugs = host_health::beacon_slugs(target, &target.name);
        claimed.extend(slugs.iter().cloned());
        // Only kind=local declares a machine that runs a beacon; "gcp" and
        // "vast" targets are dispatcher pools, not boxes.
        if !target.is_provider(crate::capabilities::ProviderId::Local) {
            continue;
        }
        // The document against itself, before any beacon is consulted: a
        // launchd domain the host cannot have is wrong whether or not the
        // host is answering, and it is the reason its beacon will never
        // report the unit.
        for misdeclared in service::misdeclared_domains(target) {
            let unit = misdeclared.unit.clone();
            findings.push(
                Finding::new("misdeclared-domain", &target.name, misdeclared.sentence())
                    .about(unit),
            );
        }
        // `managed_versions` belongs only to the compiled `host release`
        // catalog. Release-control products already carry their desired
        // version in that block, while arbitrary `service update` trees carry
        // an artifact identity instead of an invented semver contract.
        for undeclared in
            service::managed_units_without_declared_version(target, release_control.as_ref())
        {
            findings.push(
                Finding::new(
                    "undeclared-service-version",
                    &target.name,
                    undeclared.sentence(),
                )
                .about(undeclared.unit),
            );
        }
        // The same shape again, and the reason this one needed the check
        // extended rather than a second one built beside it: both loops
        // above resolve `policy.targets.get(host)` before they compare
        // anything, so a host no product target names is their skip
        // condition instead of their finding — and that host is exactly the
        // one a product's declared environment cannot reach.
        for unreachable in service::unreachable_product_environments(
            target,
            release_control.as_ref(),
            local_host.as_deref(),
        ) {
            findings.push(
                Finding::new(unreachable.kind(), &target.name, unreachable.sentence())
                    .about(unreachable.unit.clone()),
            );
        }
        // The last of the pre-beacon checks, and the one the beacon could
        // never have answered: which FILE a unit's live process is executing.
        // The beacon publishes one `state` word per unit and a unit running
        // an obsolete build is `active` by every measure it takes, so this is
        // read off the process table and the kernel rather than out of the
        // store — and therefore only on the host this command runs on.
        //
        // `com.wisent.compute.disk-cleanup.disk-cleanup` spent thirteen days
        // journalling `policy:ValueError` on 8,348 passes from a `--watch`
        // process that had been alive since 27 August, executing an image the
        // file underneath it no longer held. Nothing revisited it, because
        // `self_update::recycle_replaced_units` cycles a unit only inside the
        // invocation that replaced its bytes; an unrelated restart is what
        // ended it.
        // The revisit ledger is one host-wide file answering one question, so
        // it is opened once for the whole pass rather than once per finding.
        // `None` unless this is the local host and some product authorised a
        // unit on it, which is no host today.
        let revisit =
            crate::release_unit_image::annotations(&document, &target.name, local_host.as_deref());
        for image in
            service::units_running_replaced_images(target, local_host.as_deref(), now.timestamp())
                .await
        {
            // The row that told an operator to restart the unit by hand is
            // the row that has to say the release agent already tried and
            // what came back. Same kind, same sentence, one clause longer: a
            // repair that happens silently is the same defect as a failure
            // that happens silently, and a new severity word for it would be
            // a third vocabulary for one condition. Only for units an enabled
            // policy explicitly owns.
            let mut sentence = image.sentence();
            if let Some(clause) = revisit.as_ref().and_then(|revisit| revisit.clause(&image)) {
                sentence.push_str(&clause);
            }
            let mut finding = Finding::new(image.kind(), &target.name, sentence);
            // The row that reports a whole host unread names no unit, and
            // attaching an empty label to it would let the `missing-plist`
            // de-duplication downstream match on the empty string.
            if !image.unit.is_empty() {
                finding = finding.about(image.unit.clone());
            }
            findings.push(finding);
        }
        // The condition that opened both silent windows, and the one no check
        // above can see: the build is refusing the document. Nothing had been
        // replaced when either window opened — the installed file and the
        // running image agreed, which is why `units_running_replaced_images`
        // fires nothing — and the REGISTRY was what moved. `resolver status`
        // learned to publish this for the resolver's own process (#345); this
        // is the same fault measured for the build an operator is holding,
        // on the surface that carries every other kind of drift.
        //
        // Local-only, and every other machine gets its unmeasured row, for
        // the reason `observe_unit_images` states about pids: a build's
        // verdict is knowable only by running that build.
        for skew in
            targets::builds_refusing_registry(&target.name, &document, local_host.as_deref())
        {
            findings.push(Finding::new(skew.kind(), &target.name, skew.sentence()));
        }
        let Some(beacon) = beacon_for_slugs(&slugs, &beacons) else {
            findings.push(Finding::new(
                "no-heartbeat",
                &target.name,
                format!(
                    "declared kind=local but no beacon exists; checked {}/{{{}}}.json",
                    host_health::HEALTH_PREFIX,
                    slugs.join(",")
                ),
            ));
            continue;
        };
        match beacon.observed_at() {
            Some(observed) => {
                let age = now - observed;
                if age.num_seconds() > stale_after_seconds() {
                    findings.push(Finding::new(
                        "stale-beacon",
                        &target.name,
                        format!(
                            "{} last updated {} ago ({}), past the {}s liveness window",
                            beacon.path,
                            human_age(age),
                            observed.to_rfc3339(),
                            stale_after_seconds()
                        ),
                    ));
                }
            }
            None => findings.push(Finding::new(
                "stale-beacon",
                &target.name,
                format!(
                    "{} carries neither an object timestamp nor reported_at",
                    beacon.path
                ),
            )),
        }
        for declared in declared_units(target, release_control.as_ref()) {
            match beacon.unit_state(&declared.id) {
                None => findings.push(
                    Finding::new(
                        "missing-plist",
                        &target.name,
                        format!(
                            "registry declares service {} ({}) but {} reports no such unit",
                            declared.name, declared.id, beacon.path
                        ),
                    )
                    .about(declared.id.as_str()),
                ),
                Some(state)
                    if state != ACTIVE_STATE && !beacon.scheduled_unit_is_healthy(&declared.id) =>
                {
                    findings.push(
                        Finding::new(
                            "unit-not-active",
                            &target.name,
                            format!(
                                "registry declares service {} ({}) but {} reports state={state}",
                                declared.name, declared.id, beacon.path
                            ),
                        )
                        .about(declared.id.as_str()),
                    )
                }
                Some(_) => {}
            }
        }
    }

    for (slug, beacon) in &beacons {
        if !claimed.contains(slug) {
            findings.push(Finding::new(
                "unmanaged-host",
                slug,
                format!(
                    "{} is publishing beacons but no registry target claims that identity",
                    beacon.path
                ),
            ));
        }
    }

    for (consumer_id, payload) in &consumers {
        // Only local agents map one-to-one onto a registry box; a "gcp" or
        // "vast" broadcast comes from an ephemeral VM the registry
        // deliberately does not enumerate.
        if !payload
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| crate::capabilities::ProviderId::Local.matches(kind))
        {
            continue;
        }
        // consumer_id is "<kind>-<hostname>"
        // (`queue::capacity::publish_capacity`); `scheduler::makespan`
        // splits it exactly this way.
        let host = consumer_id
            .split_once('-')
            .map_or(consumer_id.as_str(), |(_, host)| host);
        let declared = registry
            .lookup_self(host)
            .map_err(|exc| CmdError::click(exc.to_string()))?
            .is_some();
        if !declared {
            findings.push(Finding::new(
                "unmanaged-agent",
                consumer_id,
                format!(
                    "broadcasting live capacity as host {host}, which no registry target declares"
                ),
            ));
        }
    }

    // Three declaration checks the beacons cannot answer. They compare the
    // document with itself and with the last measurement rather than with a
    // heartbeat, so they run for every target, including the dispatcher pools
    // the liveness checks above skip.
    let entries: BTreeMap<&str, &Value> = document
        .get("targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .map(|name| (name, entry))
                })
                .collect()
        })
        .unwrap_or_default();
    for loop_back in
        crate::service_resolution::self_referencing_endpoints(&document).map_err(CmdError::click)?
    {
        findings.push(Finding::new(
            "self-referencing-endpoint",
            &loop_back.target,
            format!(
                "service_directory.services.{}.{}.{} is {}, which is {} on that same target: the \
                 adapter would proxy to itself",
                loop_back.service,
                loop_back.map,
                loop_back.target,
                loop_back.address,
                loop_back.adapter
            ),
        ));
    }

    // Which declared service runs which job. The published roster is read whether
    // or not anything declares one, because a job the roster names and no target
    // declares is itself a finding: a declared service entry is the result of a
    // placement that succeeded, so a job with no entry anywhere is a job waiting
    // for a host that can take it.
    let claims: Vec<(&ComputeTarget, RequirementClaim)> = registry
        .targets
        .iter()
        .flat_map(|target| {
            declared_trajectories(target)
                .into_iter()
                .map(move |claim| (target, claim))
        })
        .collect();
    let mut measured_hosts = usize::default();
    let mut roster = usize::default();
    // An unreadable prefix is not an absent object, and reporting it as one would
    // say a host cannot do something when the truth is that nobody here may look.
    // Both reads share that reasoning, so both report the store's own words.
    match (
        load_job_requirements(&store, now).await,
        load_capability_measurements(&store).await,
    ) {
        (Ok(declarations), Ok(measurements)) => {
            measured_hosts = measurements.len();
            roster = declarations.needs.len();
            for (target, claim) in &claims {
                for reason in unmet_requirements(
                    &target.name,
                    claim,
                    &declarations,
                    measurements.get(&target.name),
                    now,
                ) {
                    findings.push(
                        Finding::new("capability-unsatisfied", &target.name, reason)
                            .about(claim.label.as_str()),
                    );
                }
            }
            let placed: BTreeSet<&str> = claims
                .iter()
                .map(|(_, claim)| claim.trajectory.as_str())
                .collect();
            findings.extend(unplaced_jobs(
                &registry,
                &declarations,
                &measurements,
                &placed,
                now,
            ));
        }
        // One row, not one per claim: the cause is a store that will not answer,
        // and it is the same cause for every job.
        (declarations, measurements) => {
            let refused = [
                declarations
                    .err()
                    .map(|exc| format!("{REQUIREMENTS_PREFIX}/ could not be read: {exc}")),
                measurements
                    .err()
                    .map(|exc| format!("{CAPABILITIES_PREFIX}/ could not be read: {exc}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join("; ");
            findings.push(Finding::new(
                "capability-unsatisfied",
                format!("{REQUIREMENTS_PREFIX}/"),
                format!(
                    "{} declared trajectory claim(s) cannot be judged and no job can be placed: \
                     {refused}",
                    claims.len()
                ),
            ));
        }
    }

    for target in &registry.targets {
        let empty = Value::Null;
        let entry = entries.get(target.name.as_str()).copied().unwrap_or(&empty);
        findings.extend(unread_declarations(target, entry));
    }
    findings.extend(unread_configuration());

    // One cause, one row. A unit the beacon does not report on a host that cannot
    // satisfy what the unit needs is already reported as `capability-unsatisfied`,
    // and the `missing-plist` row for the same label is that finding's symptom:
    // installing the plist would not make the host able to run it. A unit
    // declared in a domain its host cannot have is the same relationship —
    // `misdeclared-domain` is why nothing loads it and why no beacon reports
    // it, and installing the plist where it is declared would change neither.
    let caused: Vec<(String, String)> = findings
        .iter()
        .filter(|finding| {
            matches!(
                finding.kind,
                "capability-unsatisfied" | "misdeclared-domain"
            )
        })
        .filter_map(|finding| {
            finding
                .unit
                .clone()
                .map(|unit| (finding.subject.clone(), unit))
        })
        .collect();
    findings.retain(|finding| {
        if finding.kind != "missing-plist" {
            return true;
        }
        let Some(unit) = finding.unit.as_deref() else {
            return true;
        };
        !caused
            .iter()
            .any(|(subject, symptom)| subject == &finding.subject && symptom == unit)
    });

    let location = targets::registry_location();
    if as_json {
        echo_json(&json!({
            "registry": location,
            "ok": findings.is_empty(),
            "checked": {
                "targets": registry.targets.len(),
                "beacons": beacons.len(),
                "capacity_consumers": consumers.len(),
                "requirement_claims": claims.len(),
                "declared_trajectories": roster,
                "capability_measurements": measured_hosts,
            },
            "findings": findings.iter().map(Finding::to_json).collect::<Vec<Value>>(),
        }));
    } else if findings.is_empty() {
        println!(
            "registry {location} agrees with live host state ({} targets, {} beacons, \
             {} live consumers)",
            registry.targets.len(),
            beacons.len(),
            consumers.len()
        );
    } else {
        let rows: Vec<Vec<String>> = findings
            .iter()
            .map(|finding| {
                vec![
                    finding.kind.to_string(),
                    finding.subject.clone(),
                    finding.detail.clone(),
                ]
            })
            .collect();
        table::print(&["FINDING", "SUBJECT", "DETAIL"], &rows);
    }
    if findings.is_empty() {
        return Ok(());
    }
    // A negative verdict, not a fault: the command ran, read everything it
    // reads, and answered. Dressed as a `CmdError::click` it was classified
    // by its own wording, so an operator was told the command failed and we
    // could not attribute it to anything but their request or credentials
    // [unknown] — four false claims about a check that worked. The same
    // silent exit `release status`, `host software`, `resolver status` and
    // `web route` verdicts use carries the one thing a gate owes its caller:
    // a non-zero code, and the count beside the two things compared.
    eprintln!(
        "{} divergence(s) between {location} and live host state; each is named above",
        findings.len()
    );
    Err(CmdError::silent(super::CLICK_ERROR_CODE))
}

// ---------------------------------------------------------------------------
// registry beacon-age
// ---------------------------------------------------------------------------

/// Sort rank, worst first. Derived `Ord` follows declaration order, so a
/// host that never reported outranks a stale one and a target that is not
/// supposed to have a beacon sinks to the bottom.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BeaconRank {
    /// kind=local with no beacon object at all.
    Missing,
    /// Has a beacon; ordered oldest-first within the rank.
    Reported,
    /// Not a machine (kind=gcp / kind=vast): no beacon is expected.
    NotExpected,
}

impl BeaconRank {
    fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Reported => "reported",
            Self::NotExpected => "not-applicable",
        }
    }
}

/// One registry host paired with the beacon that proves it is alive.
struct BeaconRow {
    name: String,
    kind: String,
    rank: BeaconRank,
    observed: Option<DateTime<Utc>>,
    reported_at: Option<String>,
    path: Option<String>,
}

/// Largest whole unit of an age, e.g. `5d` — the "has not reported in
/// days" signal at a glance. chrono performs every unit conversion, so no
/// seconds-per-day arithmetic is hand-rolled here.
///
/// Shared with `cli::host::ping`, which grades the same beacon for a
/// single host: one spelling of an age across both commands.
pub(crate) fn human_age(age: TimeDelta) -> String {
    for (amount, suffix) in [
        (age.num_days(), "d"),
        (age.num_hours(), "h"),
        (age.num_minutes(), "m"),
    ] {
        if amount.is_positive() {
            return format!("{amount}{suffix}");
        }
    }
    format!("{}s", age.num_seconds().max(i64::default()))
}

/// `stado registry beacon-age [--json]` — every registry host and its last
/// beacon, worst first.
///
/// Lists hosts with no beacon at all: a machine that silently stopped
/// reporting is exactly what this table exists to surface, and a row that
/// is absent surfaces nothing.
pub async fn beacon_age(as_json: bool) -> Result<(), CmdError> {
    let (document, _) = fetch_versioned_document().await?;
    let registry = targets::load_registry_from_value(&document).map_err(|error| {
        CmdError::click(format!(
            "invalid registry document at {}: {error}",
            targets::registry_location()
        ))
    })?;
    let store = JobStorage::for_primary_reads().await?;
    let beacons = load_beacons(&store).await?;
    let now = Utc::now();

    let mut rows: Vec<BeaconRow> = registry
        .targets
        .iter()
        .map(|target| {
            let beacon = beacon_for(target, &beacons);
            let rank = if beacon.is_some() {
                BeaconRank::Reported
            } else if target.is_provider(crate::capabilities::ProviderId::Local) {
                BeaconRank::Missing
            } else {
                BeaconRank::NotExpected
            };
            BeaconRow {
                name: target.name.clone(),
                kind: target.kind.clone(),
                rank,
                observed: beacon.and_then(Beacon::observed_at),
                reported_at: beacon.and_then(Beacon::reported_at).map(str::to_string),
                path: beacon.map(|beacon| beacon.path.clone()),
            }
        })
        .collect();
    // (rank, observed) ascending: never-reported first, then the oldest
    // beacon, with the not-applicable rows last.
    rows.sort_by_key(|row| (row.rank, row.observed));

    let age_of = |row: &BeaconRow| row.observed.map(|observed| now - observed);
    if as_json {
        let hosts: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "host": row.name,
                    "kind": row.kind,
                    "status": row.rank.label(),
                    "beacon": row.path,
                    "observed_at": row.observed.map(|ts| ts.to_rfc3339()),
                    "reported_at": row.reported_at,
                    "age_seconds": age_of(row).map(|age| age.num_seconds()),
                })
            })
            .collect();
        echo_json(&json!({"registry": targets::registry_location(), "hosts": hosts}));
        return Ok(());
    }

    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let age = match (row.rank, age_of(row)) {
                (BeaconRank::Missing, _) => "never".to_string(),
                (BeaconRank::NotExpected, None) => "-".to_string(),
                (_, Some(age)) => human_age(age),
                (_, None) => "unknown".to_string(),
            };
            vec![
                row.name.clone(),
                row.kind.clone(),
                age,
                row.observed
                    .map_or_else(|| "-".to_string(), |ts| ts.to_rfc3339()),
                row.reported_at.clone().unwrap_or_else(|| "-".to_string()),
                row.path.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    table::print(
        &["HOST", "KIND", "AGE", "OBSERVED", "REPORTED_AT", "BEACON"],
        &table_rows,
    );
    Ok(())
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`.
fn echo_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(exc) => eprintln!("could not render json: {exc}"),
    }
}
