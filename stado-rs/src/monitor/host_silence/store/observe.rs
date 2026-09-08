//! The open/close transition, written by whoever notices.

use chrono::{DateTime, Utc};

use crate::monitor::host_silence::paths::silence_object_path;
use crate::monitor::host_silence::records::SilenceRecord;
use crate::monitor::host_silence::transitions::{
    beacon_is_silent, close_record, merge_observation, open_record, silence_threshold_seconds,
};
use crate::queue::{JobStorage, StorageError};

use super::reads::{open_silence, read_document};

/// Fold one observation of a host's newest beacon into its silence record.
///
/// The single entry point for the open/close transition, called by whatever
/// component happens to look at a beacon: `stado host link`, the resolver,
/// the dashboard. Whichever one notices the threshold crossing writes it,
/// which is why the record is keyed by `started_at` and created
/// conditionally — three readers noticing the same outage produce one
/// record with three names in `observed_by`, not three records.
///
/// Returns the record this call wrote, or `None` when there was nothing to
/// write (host healthy and no gap open, or an open gap this observer had
/// already been recorded in).
pub async fn observe_beacon_age_at(
    store: &JobStorage,
    host: &str,
    newest_beacon_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    threshold_seconds: i64,
    observer: &str,
    first_reader_error: Option<&str>,
) -> Result<Option<SilenceRecord>, StorageError> {
    let existing = open_silence(store, host).await?;
    if beacon_is_silent(newest_beacon_at, now, threshold_seconds) {
        let Some((path, mut record)) = existing else {
            // A host that has never published has no last-heard-from
            // instant, so the gap starts now: claiming it started at the
            // epoch would report an outage nobody lived through.
            let started_at = newest_beacon_at.unwrap_or(now);
            let record = open_record(host, started_at, observer, first_reader_error);
            let path = silence_object_path(host, started_at);
            let body = serde_json::to_string_pretty(&record)?;
            if store.create_text_if_absent(&path, &body).await? {
                return Ok(Some(record));
            }
            // Lost the create race with another observer of the same
            // crossing. Their record is the record; merge into it rather
            // than overwrite, so neither name is lost.
            let Some(mut theirs) = read_document::<SilenceRecord>(store, &path).await? else {
                return Ok(None);
            };
            if !merge_observation(&mut theirs, observer, first_reader_error) {
                return Ok(None);
            }
            store
                .upload_text(&path, &serde_json::to_string_pretty(&theirs)?)
                .await?;
            return Ok(Some(theirs));
        };
        if !merge_observation(&mut record, observer, first_reader_error) {
            return Ok(None);
        }
        store
            .upload_text(&path, &serde_json::to_string_pretty(&record)?)
            .await?;
        return Ok(Some(record));
    }
    let Some((path, mut record)) = existing else {
        return Ok(None);
    };
    // The gap ended when the host published again, not when somebody got
    // around to looking: `ended_at` is the fresher beacon's own instant, so
    // `duration_seconds` is the outage and not the polling interval.
    let ended_at = newest_beacon_at.unwrap_or(now);
    merge_observation(&mut record, observer, first_reader_error);
    if !close_record(&mut record, ended_at) {
        return Ok(None);
    }
    store
        .upload_text(&path, &serde_json::to_string_pretty(&record)?)
        .await?;
    Ok(Some(record))
}

/// [`observe_beacon_age_at`] at the current instant, with the fleet-wide
/// threshold from [`silence_threshold_seconds`].
pub async fn observe_beacon_age(
    store: &JobStorage,
    host: &str,
    newest_beacon_at: Option<DateTime<Utc>>,
    observer: &str,
    first_reader_error: Option<&str>,
) -> Result<Option<SilenceRecord>, StorageError> {
    observe_beacon_age_at(
        store,
        host,
        newest_beacon_at,
        Utc::now(),
        silence_threshold_seconds(),
        observer,
        first_reader_error,
    )
    .await
}
