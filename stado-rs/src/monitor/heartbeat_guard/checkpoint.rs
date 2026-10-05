//! The GCS checkpoint prefix: where a training command writes it, and the
//! newest write under it — proof the job is alive even while its lease
//! renewal is network-starved by the very upload that writes it.

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::queue::JobStorage;

static CKPT_URI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--checkpoint-gcs-uri\s+(\S+)").expect("static regex compiles"));

/// Extract the in-bucket checkpoint prefix from a training command's
/// `--checkpoint-gcs-uri gs://<bucket>/<prefix>` flag. Returns the part
/// after the bucket (e.g. 'ckpts/qwen3_4b_5k_s0/') or None.
fn ckpt_prefix_from_command(cmd: &str) -> Option<String> {
    if cmd.is_empty() {
        return None;
    }
    let caps = CKPT_URI_RE.captures(cmd)?;
    let uri = caps[1].trim();
    let rest = uri.strip_prefix("gs://")?;
    let (_bucket, prefix) = rest.split_once('/')?;
    if prefix.is_empty() {
        return None;
    }
    Some(prefix.to_string())
}

/// Whether the job's checkpoint directory received a write after `since`.
///
/// While a multi-GB checkpoint uploads its shard blobs are continuously
/// updated, so the newest write keeps moving even while the upload starves
/// the small lease renewal. A listing the coordinator cannot read is not
/// proof the job is dead, so it answers true (keep).
pub(super) async fn checkpoint_written_after(
    store: &JobStorage,
    command: &str,
    since: DateTime<Utc>,
) -> bool {
    let Some(prefix) = ckpt_prefix_from_command(command) else {
        return false;
    };
    match store.list_blobs_with_meta(&prefix).await {
        Ok(infos) => infos
            .into_iter()
            .filter_map(|info| info.updated)
            .any(|updated| updated > since),
        Err(_) => true,
    }
}
