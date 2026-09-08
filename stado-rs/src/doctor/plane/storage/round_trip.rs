//! The one write `stado doctor` performs, and what reading it back proves.

use chrono::{SecondsFormat, Utc};
use serde_json::json;

use crate::doctor::{Check, Findings, Status, PROBE_PREFIX};
use crate::providers;
use crate::queue::JobStorage;
use crate::targets;

pub(in crate::doctor) const STORAGE_ID: &str = "storage";
pub(in crate::doctor) const STORAGE_TITLE: &str = "Storage round trip";
pub(in crate::doctor) const STORAGE_REMEDY: &str =
    "the store is selected by WC_STORAGE_BACKEND and its locator vars; provider \
     adapters use their configured workload identity and never fall back to a cloud CLI \
     or a provider-key grant";

/// Object name for one round trip. Unique per run, so two operators
/// running `stado doctor` at once cannot delete each other's probe, and
/// named so whoever finds a leaked one knows what it is without grepping.
fn probe_blob() -> String {
    let host = targets::normalize_hostname(&providers::vast::system_hostname());
    format!(
        "{PROBE_PREFIX}stado-doctor-probe-safe-to-delete-{host}-{}.json",
        uuid::Uuid::new_v4()
    )
}

/// Write, read back and delete one object. The only write `stado doctor`
/// performs, and the only check anywhere that separates "the queue is
/// empty" from "the store is unreachable".
pub(in crate::doctor) async fn check_storage_round_trip(
    store: Option<&JobStorage>,
    store_error: &str,
) -> Check {
    let Some(store) = store else {
        return Check::fail(
            STORAGE_ID,
            STORAGE_TITLE,
            format!("storage backend could not be constructed: {store_error}"),
            STORAGE_REMEDY,
        );
    };
    let target = format!(
        "backend={} bucket={:?}",
        store.backend_name(),
        store.bucket_name()
    );
    let path = probe_blob();
    let payload = serde_json::to_string_pretty(&json!({
        "written_by": "stado doctor",
        "purpose": "storage auth + round-trip probe",
        "note": "diagnostic only, carries no queue state; safe to delete",
        "written_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        "host": providers::vast::system_hostname(),
    }))
    .expect("probe document serializes");

    // Cleanup is unconditional once a write was attempted: a doctor that
    // leaks an object on every failing run is worse than no doctor. Delete
    // is idempotent, so running it after a failed write costs nothing.
    let written = store.upload_text(&path, &payload).await;
    let read_back = match written {
        Ok(()) => Some(store.download_text(&path).await),
        Err(_) => None,
    };
    let cleaned = store.delete_blob(&path).await;

    let mut findings = Findings::default();
    match (written, read_back) {
        (Err(err), _) => {
            findings.note(
                Status::Fail,
                format!("write of {path} failed ({target}): {err}"),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), Some(Err(err))) => {
            findings.note(
                Status::Fail,
                format!("read back of {path} failed ({target}): {err}"),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), Some(Ok(None))) => {
            findings.note(
                Status::Fail,
                format!(
                    "{path} was written without error but reads back as absent ({target}); the \
                     credentials can write but not read, or the write landed somewhere other \
                     than where the read looks"
                ),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), Some(Ok(Some(text)))) if text != payload => {
            findings.note(
                Status::Fail,
                format!(
                    "{path} read back {} byte(s) instead of the {} written ({target}); the \
                     backend is not returning what it stored",
                    text.len(),
                    payload.len()
                ),
            );
            findings.remedy(STORAGE_REMEDY);
        }
        (Ok(()), _) => findings.note(
            Status::Pass,
            format!("wrote, read back and deleted {path} ({target})"),
        ),
    }
    if let Err(err) = cleaned {
        findings.note(
            Status::Warn,
            format!("probe object {path} could NOT be deleted and is still in the store: {err}"),
        );
        findings.remedy(format!("delete the leaked probe object {path} by hand"));
    }
    findings.into_check(STORAGE_ID, STORAGE_TITLE, STORAGE_REMEDY)
}
