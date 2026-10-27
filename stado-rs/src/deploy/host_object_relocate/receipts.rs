//! What the relocation script reports, line by line, and the reading it folds
//! into.

use crate::deploy::host_channel;

/// One object the pass judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    pub outcome: String,
    pub bytes: i64,
    pub sha256: Option<String>,
    pub source_key: String,
    pub destination_key: String,
}

/// One sidecar the pass moved or refused.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Marker lines the host printed that this build could not read: the
    /// line and why. A counter that did not parse is here, never totalled
    /// as zero, and the closing marker counts as complete only when every
    /// one of its counters read.
    pub unreadable: Vec<String>,
}

/// One counter the host printed, or the sentence saying why it is not one.
fn counter(line: &str, name: &str, text: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|error| format!("{line}: {name} {text:?} is not a count: {error}"))
}

/// Fold the marker lines of stdout into a reading.
pub fn parse_output(stdout: &str) -> RelocateReading {
    let mut reading = RelocateReading::default();
    for line in stdout.lines() {
        let folded = match host_channel::marker_fields(line).as_slice() {
            ["STADO_RELOCATE_NO_ROOT", root] => {
                reading.missing_root = Some((*root).to_string());
                Ok(())
            }
            ["STADO_RELOCATE_NO_HASHER", os] => {
                reading.no_hasher = Some((*os).to_string());
                Ok(())
            }
            ["STADO_RELOCATE_ROOT", root, source, destination] => {
                reading.store_root = Some((*root).to_string());
                reading.source_prefix = Some((*source).to_string());
                reading.destination_prefix = Some((*destination).to_string());
                Ok(())
            }
            ["STADO_RELOCATE", outcome, bytes, sha, source, destination] => {
                counter(line, "bytes", bytes).map(|bytes| {
                    reading.objects.push(Relocation {
                        outcome: (*outcome).to_string(),
                        bytes,
                        sha256: match *sha {
                            "-" | "" => None,
                            value => Some(value.to_string()),
                        },
                        source_key: (*source).to_string(),
                        destination_key: (*destination).to_string(),
                    });
                })
            }
            ["STADO_RELOCATE_META", outcome, source] => {
                reading.metadata.push(MetadataMove {
                    outcome: (*outcome).to_string(),
                    source_key: (*source).to_string(),
                });
                Ok(())
            }
            ["STADO_RELOCATE_PRUNED", count] => {
                counter(line, "pruned", count).map(|count| reading.pruned_directories = count)
            }
            ["STADO_RELOCATE_STALE_URI", stale, repaired] => counter(line, "stale", stale)
                .and_then(|stale| {
                    counter(line, "repaired", repaired).map(|repaired| (stale, repaired))
                })
                .map(|(stale, repaired)| {
                    reading.stale_uris = stale;
                    reading.repaired_uris = repaired;
                }),
            ["STADO_RELOCATE_END", scanned, decided, moved, moved_bytes, refused] => {
                let totals = (|| {
                    Ok::<_, String>((
                        counter(line, "scanned", scanned)?,
                        counter(line, "decided", decided)?,
                        counter(line, "moved", moved)?,
                        counter(line, "moved_bytes", moved_bytes)?,
                        counter(line, "refused", refused)?,
                    ))
                })();
                totals.map(|(scanned, decided, moved, moved_bytes, refused)| {
                    reading.scanned = scanned;
                    reading.decided = decided;
                    reading.moved = moved;
                    reading.moved_bytes = moved_bytes;
                    reading.refused = refused;
                    reading.complete = true;
                })
            }
            _not_a_marker => Ok(()),
        };
        if let Err(unreadable) = folded {
            reading.unreadable.push(unreadable);
        }
    }
    reading
}
