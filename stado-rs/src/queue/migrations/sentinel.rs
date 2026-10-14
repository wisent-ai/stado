//! The resumable record the sweep leaves behind: where it stopped, whether
//! the prefix has been covered end-to-end, and which coverage that answer
//! belongs to.

use crate::queue::storage::JobStorage;
use crate::queue::{python_json_dumps, StorageError};

/// Python `_SENTINEL_PATH`.
pub const SENTINEL_PATH: &str = "queue_priority/.migration.json";

/// The coverage a completed sentinel attests to.
///
/// Coverage includes every queued job under its full-width integer priority
/// key. A sentinel for the former decimal encoding does not establish this
/// order: the existing backfill must write current keys and prune superseded
/// names before any index-only walk can use them.
const COVERAGE: &str = "all-queued-integer-priority";

/// Python sentinel dict `{"cursor": str, "done": bool}`, plus the coverage
/// the `done` belongs to.
///
/// `done` no longer means "this migration is over and must never run again".
/// It means "the sweep has covered the prefix end-to-end at least once", so
/// the whole-prefix pass in [`super::listing::list_claimable`] can be
/// switched off. It is sticky across the cursor's rewind: the sweep keeps
/// running forever, the whole-prefix pass stays retired.
///
/// [`super::listing::list_claimable`]: crate::queue::listing::list_claimable
pub(super) struct Sentinel {
    pub(super) done: bool,
}

/// Python `_read_sentinel`, with the coverage check folded in.
///
/// A sentinel written before the index covered every queued job carries no
/// `coverage` key; its `done` and its `cursor` both describe the narrower
/// pass, so neither is usable and the walk restarts from the head under the
/// current coverage.
pub(super) async fn read_sentinel(store: &JobStorage) -> Result<Sentinel, StorageError> {
    let fresh = Sentinel { done: false };
    let Some(raw) = store.download_text(SENTINEL_PATH).await? else {
        return Ok(fresh);
    };
    if raw.is_empty() {
        return Ok(fresh);
    }
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    if value.get("coverage").and_then(serde_json::Value::as_str) != Some(COVERAGE) {
        return Ok(fresh);
    }
    Ok(Sentinel {
        done: value
            .get("done")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// Whether some sweep has covered `queue/` end-to-end under the current
/// coverage. One small download, and the only question
/// [`super::listing::list_claimable`] needs answered per poll.
///
/// Deliberately separate from [`backfill_priority_markers`]: coverage is
/// cheap to read, the repair that establishes it is not, and conflating them
/// made every poll pay a sweep. A sentinel from the narrower priority>0 era
/// reads as not-swept, exactly as the walk requires.
///
/// [`super::listing::list_claimable`]: crate::queue::listing::list_claimable
/// [`backfill_priority_markers`]: super::backfill_priority_markers
pub async fn has_swept(store: &JobStorage) -> Result<bool, StorageError> {
    Ok(read_sentinel(store).await?.done)
}

/// Python `_write_sentinel` (`json.dumps({"cursor": ..., "done": ...})`
/// with default separators), stamped with the coverage its `done` attests to.
pub(super) async fn write_sentinel(
    store: &JobStorage,
    cursor: &str,
    done: bool,
) -> Result<(), StorageError> {
    let body = python_json_dumps(
        &serde_json::json!({"cursor": cursor, "done": done, "coverage": COVERAGE}),
    )?;
    store.upload_text(SENTINEL_PATH, &body).await
}
