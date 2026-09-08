//! `stado fleet invites` and the label it prints, which has to say what an
//! open offline invite is actually waiting for.

use serde_json::json;

use crate::queue::JobStorage;

use super::store::list_invites;
use super::{Invite, MODE_OFFLINE, STATUS_OPEN};

/// An open offline invite is not waiting for a redeemer, it is waiting for the
/// machine's owner to send back an address — nothing will ever spend it by
/// itself. The list says so where the state goes, because "open" alone reads
/// like the online mode, where somebody may be about to run the one-liner.
/// Pure.
pub fn status_label(invite: &Invite, status: &str) -> String {
    if invite.mode != MODE_OFFLINE {
        return status.to_string();
    }
    if status == STATUS_OPEN {
        "open (offline, awaiting address)".to_string()
    } else {
        format!("{status} (offline)")
    }
}

/// `stado fleet invites` — every invite and the state it is actually in.
pub async fn invites(as_json: bool) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let found = list_invites(&store).await?;
    if as_json {
        let rendered = json!({
            "invites": found
                .iter()
                .map(|(invite, status)| json!({
                    "id": invite.id,
                    "target_name": invite.target_name,
                    "status": status,
                    "mode": invite.mode,
                    "awaiting_address": invite.mode == MODE_OFFLINE && *status == STATUS_OPEN,
                    "created_at": invite.created_at,
                    "expires_at": invite.expires_at,
                    "uses_allowed": invite.uses_allowed,
                    "uses_spent": invite.uses_spent,
                    "created_by": invite.created_by,
                }))
                .collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    if found.is_empty() {
        println!("no invites");
        return Ok(true);
    }
    for (invite, status) in &found {
        println!(
            "{}\t{}\t{}\t{}/{}\texpires {}",
            invite.id,
            invite.target_name,
            status_label(invite, status),
            invite.uses_spent,
            invite.uses_allowed,
            invite.expires_at
        );
    }
    if found
        .iter()
        .any(|(invite, status)| invite.mode == MODE_OFFLINE && *status == STATUS_OPEN)
    {
        println!(
            "an offline invite closes when its machine is registered: stado fleet enroll NAME --ssh <address> --bootstrap"
        );
    }
    Ok(true)
}
