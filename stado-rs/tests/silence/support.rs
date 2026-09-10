//! The real local storage every silence story writes to, the two sentences
//! the incident produced, and the small readers each test uses to look at
//! what landed on disk.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use stado::monitor::host_silence::{refusal_object_path, RefusalRecord};
use stado::queue::{JobStorage, LocalBackend};

pub(crate) const HOST: &str = "control-host";

/// The resolver's own sentence from the incident, verbatim.
pub(crate) const AUTHORITY_SENTENCE: &str = "registry authority exited with exit status: 255: ssh: connect to host 10.0.0.253 port 22: Operation timed out";

/// The other reader's own sentence from the incident, verbatim.
pub(crate) const STALE_SENTENCE: &str = "service directory cache is stale (store generation 7)";
pub(crate) fn store(root: &Path) -> JobStorage {
    let backend = LocalBackend::new(root.to_str().expect("tempdir path is utf-8"))
        .expect("local backend roots at the tempdir");
    JobStorage::with_backend(Arc::new(backend), "local")
}

pub(crate) fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("fixture timestamp is RFC 3339")
        .with_timezone(&Utc)
}

/// The document actually on disk at `blob`, parsed.
pub(crate) fn on_disk(root: &Path, blob: &str) -> Value {
    let body = std::fs::read_to_string(root.join(blob))
        .unwrap_or_else(|error| panic!("{blob} is not on disk: {error}"));
    serde_json::from_str(&body).expect("the record stays JSON")
}

/// Blob names directly under `dir`, sorted.
pub(crate) fn blob_names(root: &Path, dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Seed one refusal at an exact instant, the way a reader publishes it.
///
/// `record_refusal` stamps `Utc::now()` and throttles, which is right for a
/// live reader and useless for building a window with known ages; the path
/// and body here are the ones it would have written.
pub(crate) async fn seed_refusal(store: &JobStorage, record: &RefusalRecord) {
    store
        .upload_text(
            &refusal_object_path(&record.host, record.at),
            &serde_json::to_string_pretty(record).expect("refusal serializes"),
        )
        .await
        .expect("seeding a refusal writes");
}

pub(crate) fn refusal(at_text: &str, reader: &str, reason: &str, detail: &str) -> RefusalRecord {
    RefusalRecord {
        host: HOST.to_string(),
        at: at(at_text),
        reader: reader.to_string(),
        reason: reason.to_string(),
        detail: detail.to_string(),
    }
}
