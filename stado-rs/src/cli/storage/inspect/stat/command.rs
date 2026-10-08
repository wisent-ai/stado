//! The command itself, and the receipt its --json form writes.

use crate::cli::storage::*;

#[derive(Args, Debug)]
pub struct StorageStatArgs {
    /// Full object name, for example `queue/<job_id>.json` or
    /// `registry.json`.
    path: String,
    #[arg(long)]
    json: bool,
}

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

/// Exit code contract: zero means the question was ANSWERED (`present` or
/// `absent`), non-zero means it was not (`refused`, `unavailable`,
/// `unreachable`). Scripting an "is it gone?" check on the exit status
/// therefore never mistakes a store that could not answer for a drained one.
///
/// Branch on `state` for which of the five it was. The three non-zero states
/// used to be one word, so a caller that wanted to retry a transient outage,
/// or to stop and fix a credential, had to grep a prose detail line to tell
/// which it was looking at.
pub(in crate::cli::storage) async fn stat(args: &StorageStatArgs) -> Result<(), CmdError> {
    // Which store answers, and how, is decided once in `probe::observe`, the
    // same question a release hand-off asks about each pinned input.
    let super::probe::Observation {
        presence,
        bucket: store_bucket,
        backend: store_backend,
        metadata,
        updated,
        metadata_error,
    } = super::probe::observe(&args.path).await?;

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
    let mut unanswered = CmdError::click(format!(
        "{}{}",
        presence.unanswered_sentence(&args.path),
        inferred_namespace_hint(&args.path)
    ));
    unanswered.failure = presence.failure();
    Err(unanswered)
}
