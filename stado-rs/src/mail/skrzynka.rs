//! Messages as Skrzynka holds them.
//!
//! Skrzynka is the product that receives mail: it holds the mailbox
//! credentials in Skarbiec and keeps each mailbox's messages in its own
//! store. Stado reads that store through Skrzynka's CLI and never talks to a
//! mail provider itself.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::MailError;

/// The installed Skrzynka, found on `PATH` like every other product CLI.
const SKRZYNKA: &str = "skrzynka";

/// One message as `skrzynka message list` prints it. Only the fields the
/// analysis reads are declared; Skrzynka's other fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct SkrzynkaMessage {
    pub id: String,
    pub mailbox_id: String,
    pub sender: String,
    pub recipients: String,
    pub subject: String,
    pub sent_at: Option<String>,
    pub received_at: String,
    pub body_text: String,
    pub snippet: String,
}

/// Every message across every enabled mailbox received at or after `since`.
/// Nothing is synchronized or changed: this reads what Skrzynka has already
/// received. Skrzynka answers newest first in pages of its own size, so pages
/// are read until one is empty or reaches a message older than `since`; no
/// count of messages is assumed.
pub async fn messages_since(since: DateTime<Utc>) -> Result<Vec<SkrzynkaMessage>, MailError> {
    let mut found = Vec::new();
    loop {
        let page = page_at(found.len()).await?;
        let Some(oldest) = page.last() else {
            return Ok(found);
        };
        let reached_older = DateTime::parse_from_rfc3339(&oldest.received_at)
            .map_err(|error| {
                MailError::Unreadable(format!(
                    "{SKRZYNKA} message list: received_at {:?} of {}: {error}",
                    oldest.received_at, oldest.id
                ))
            })?
            .with_timezone(&Utc)
            < since;
        found.extend(page);
        if reached_older {
            return Ok(found);
        }
    }
}

/// One page of Skrzynka's newest-first listing, starting `offset` messages in.
async fn page_at(offset: usize) -> Result<Vec<SkrzynkaMessage>, MailError> {
    let output = crate::wait::output_async(
        &mut tokio::process::Command::new(SKRZYNKA)
            .args(["message", "list", "--offset", &offset.to_string()])
            .stdin(std::process::Stdio::null()),
    )
    .await
    .map_err(|error| MailError::Unreachable(format!("{SKRZYNKA} message list: {error}")))?;
    if !output.status.success() {
        return Err(MailError::Refused {
            status: output.status.to_string(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| MailError::Unreadable(format!("{SKRZYNKA} message list: {error}")))
}
