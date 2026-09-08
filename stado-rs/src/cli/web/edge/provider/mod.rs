//! The provider API calls: what the edge is made of on Azure, and what it
//! takes to remove one of those resources and know it is gone.

use std::time::Duration;

use super::{mutate_web, unit_label, CmdError, DISCARD_TIMEOUT};
use crate::providers::azure;

mod provision;
mod removal;
mod requests;

pub(in crate::cli::web::edge) use provision::provision;
pub(in crate::cli::web::edge) use removal::remove;

/// Delete one ARM resource and wait until reading it answers 404.
///
/// The provider's own delete does not wait, and here it has to: a public
/// address cannot be removed while the interface still references it, so a
/// rollback that fired three deletes at once would leave the address behind —
/// billed, unattached, and belonging to nothing. `get_allow_404` returning
/// `None` is the only evidence that the resource is actually gone.
async fn discard(client: &azure::ArmClient, path: &str, description: &str) -> Result<(), String> {
    client
        .delete_allow_404(path, description)
        .await
        .map_err(|error| error.to_string())?;
    let deadline = tokio::time::Instant::now() + DISCARD_TIMEOUT;
    loop {
        match client.get_allow_404(path, description).await {
            Ok(None) => return Ok(()),
            Ok(Some(_)) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(Some(_)) => {
                return Err(format!(
                    "{description}: still present {}s after the delete was accepted",
                    DISCARD_TIMEOUT.as_secs()
                ))
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Remove, in reverse creation order, what a failed provision created.
///
/// Every failure is collected rather than propagated: the caller is already
/// returning Azure's own refusal, and what an operator needs added to it is
/// the list of resources that are still there.
async fn unwind(client: &azure::ArmClient, created: &[(String, String)]) -> Vec<String> {
    let mut leftovers = Vec::new();
    for (path, description) in created.iter().rev() {
        if let Err(error) = discard(client, path, description).await {
            leftovers.push(error);
        }
    }
    leftovers
}

/// Azure's refusal, plus whatever the rollback could not remove.
fn refusal(context: &str, error: impl ToString, leftovers: &[String]) -> CmdError {
    let mut message = format!("{context}: {}", error.to_string());
    if !leftovers.is_empty() {
        message.push_str(&format!(
            "; these resources were created and could not be removed, so they are still \
             billing: {}",
            leftovers.join("; ")
        ));
    }
    CmdError::click(message)
}
