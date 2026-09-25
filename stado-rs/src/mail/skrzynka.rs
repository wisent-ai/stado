//! Messages as Skrzynka holds them.
//!
//! Skrzynka is the product that receives mail: it holds the mailbox
//! credentials in Skarbiec and keeps each mailbox's messages in its own
//! store. Stado reads that store through Skrzynka's CLI and never talks to a
//! mail provider itself.

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

/// The newest `limit` messages across every enabled mailbox. Nothing is
/// synchronized or changed: this reads what Skrzynka has already received.
pub async fn messages(limit: usize) -> Result<Vec<SkrzynkaMessage>, MailError> {
    let output = tokio::process::Command::new(SKRZYNKA)
        .args(["message", "list", "--limit", &limit.to_string()])
        .stdin(std::process::Stdio::null())
        .output()
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
