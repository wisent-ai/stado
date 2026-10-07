//! Persistence of the record: one invite in, one invite out, and the whole
//! shelf for the callers that report or search it.

use chrono::Utc;
use serde_json::Value;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::queue::JobStorage;

use super::{effective_status, invite_document, invite_path, parse_invite, Invite, INVITES_PREFIX};

/// A stored invite that does not parse is a damaged record in the store,
/// not a mistake in this command's input.
fn damaged(id: &str, cause: impl std::fmt::Display) -> CmdError {
    CmdError::click(format!("stored invite '{id}' does not parse: {cause}"))
        .stating(FailureCode::InfraDown)
}

pub(in crate::cli::fleet::invite) async fn load_invite(
    store: &JobStorage,
    id: &str,
) -> Result<Option<Invite>, CmdError> {
    let Some(text) = store.download_text(&invite_path(id)).await? else {
        return Ok(None);
    };
    let document: Value = serde_json::from_str(&text).map_err(|exc| damaged(id, exc))?;
    parse_invite(&document)
        .map(Some)
        .map_err(|exc| damaged(id, exc))
}

pub(in crate::cli::fleet::invite) async fn store_invite(
    store: &JobStorage,
    invite: &Invite,
) -> Result<(), CmdError> {
    store
        .upload_text(
            &invite_path(&invite.id),
            &serde_json::to_string_pretty(&invite_document(invite))?,
        )
        .await
        .map_err(CmdError::from)
}

/// Every invite in the store, newest first, each with its effective status.
pub async fn list_invites(store: &JobStorage) -> Result<Vec<(Invite, &'static str)>, CmdError> {
    let blobs = store.list_blobs_with_meta(INVITES_PREFIX).await?;
    let now = Utc::now();
    let mut found = Vec::new();
    for blob in &blobs {
        let Some(text) = store.download_text(&blob.name).await? else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Ok(invite) = parse_invite(&document) else {
            continue;
        };
        let status = effective_status(&invite, now);
        found.push((invite, status));
    }
    found.sort_by(|left, right| right.0.created_at.cmp(&left.0.created_at));
    Ok(found)
}
