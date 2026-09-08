//! The outcome vocabulary, and the receipts one pass hands back: the marker
//! lines of stdout folded into a reading.

use crate::deploy::host_channel;

/// Moved: linked, hashed, compared, source unlinked.
pub const MOVED: &str = "moved";
/// `--apply` was not given, and this object is what a pass would move.
pub const WOULD_MOVE: &str = "would_move";
/// The destination already held these exact bytes, so an earlier pass had
/// linked it and died before unlinking the source. The source is gone now.
pub const CONVERGED: &str = "converged";
/// The destination exists with OTHER content. Both copies kept, untouched.
pub const DESTINATION_DIFFERS: &str = "destination_differs";
/// The link was made and the destination did not hash to the source. The
/// destination was unlinked; the source is untouched.
pub const VERIFY_FAILED: &str = "verify_failed";
/// The host refused the link itself. Nothing changed.
pub const LINK_FAILED: &str = "link_failed";

/// Every outcome that leaves the object where it was found, in need of an
/// operator: the two `--json` consumers and the printer agree on one list
/// rather than each spelling its own.
pub fn is_refusal(outcome: &str) -> bool {
    matches!(outcome, DESTINATION_DIFFERS | VERIFY_FAILED | LINK_FAILED)
}

/// One object the pass reached a verdict about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relocation {
    pub outcome: String,
    pub bytes: i64,
    pub sha256: Option<String>,
    pub source_key: String,
    pub destination_key: String,
}

/// One sidecar that travelled, or refused to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataMove {
    pub outcome: String,
    pub source_key: String,
}

/// Everything one pass answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelocateReading {
    pub store_root: Option<String>,
    pub source_prefix: Option<String>,
    pub destination_prefix: Option<String>,
    /// The store root is not a directory on this host, so nothing was read.
    pub missing_root: Option<String>,
    /// No sha256 program, so nothing was moved: a body this command cannot
    /// verify is a body it does not touch.
    pub no_hasher: Option<String>,
    pub objects: Vec<Relocation>,
    pub metadata: Vec<MetadataMove>,
    pub scanned: i64,
    pub decided: i64,
    pub moved: i64,
    pub moved_bytes: i64,
    pub refused: i64,
    pub pruned_directories: i64,
    /// Sidecars under the destination whose recorded `stado-uri` still names
    /// the source prefix, and how many of those this pass rewrote.
    pub stale_uris: i64,
    pub repaired_uris: i64,
    /// The closing marker arrived, so the totals are the host's own and not a
    /// truncated read.
    pub complete: bool,
}

/// Fold the marker lines of stdout into a reading.
pub fn parse_output(stdout: &str) -> RelocateReading {
    let mut reading = RelocateReading::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_RELOCATE_NO_ROOT", root] => reading.missing_root = Some((*root).to_string()),
            ["STADO_RELOCATE_NO_HASHER", os] => reading.no_hasher = Some((*os).to_string()),
            ["STADO_RELOCATE_ROOT", root, source, destination] => {
                reading.store_root = Some((*root).to_string());
                reading.source_prefix = Some((*source).to_string());
                reading.destination_prefix = Some((*destination).to_string());
            }
            ["STADO_RELOCATE", outcome, bytes, sha, source, destination] => {
                reading.objects.push(Relocation {
                    outcome: (*outcome).to_string(),
                    bytes: bytes.parse::<i64>().unwrap_or_default(),
                    sha256: match *sha {
                        "-" | "" => None,
                        value => Some(value.to_string()),
                    },
                    source_key: (*source).to_string(),
                    destination_key: (*destination).to_string(),
                });
            }
            ["STADO_RELOCATE_META", outcome, source] => {
                reading.metadata.push(MetadataMove {
                    outcome: (*outcome).to_string(),
                    source_key: (*source).to_string(),
                });
            }
            ["STADO_RELOCATE_PRUNED", count] => {
                reading.pruned_directories = count.parse::<i64>().unwrap_or_default();
            }
            ["STADO_RELOCATE_STALE_URI", stale, repaired] => {
                reading.stale_uris = stale.parse::<i64>().unwrap_or_default();
                reading.repaired_uris = repaired.parse::<i64>().unwrap_or_default();
            }
            ["STADO_RELOCATE_END", scanned, decided, moved, moved_bytes, refused] => {
                reading.scanned = scanned.parse::<i64>().unwrap_or_default();
                reading.decided = decided.parse::<i64>().unwrap_or_default();
                reading.moved = moved.parse::<i64>().unwrap_or_default();
                reading.moved_bytes = moved_bytes.parse::<i64>().unwrap_or_default();
                reading.refused = refused.parse::<i64>().unwrap_or_default();
                reading.complete = true;
            }
            _ => {}
        }
    }
    reading
}
