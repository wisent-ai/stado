//! The operator's refusal: retire an invite so no redemption of it can
//! succeed, and say what it was before.

use chrono::Utc;

use crate::cli::fleet::invite::record::store::{load_invite, store_invite};
use crate::cli::fleet::invite::record::{effective_status, STATUS_REVOKED};
use crate::queue::JobStorage;

/// `stado fleet revoke-invite ID` — retire an invite before anybody uses it.
pub async fn revoke_invite(id: &str) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let mut invite = load_invite(&store, id)
        .await?
        .ok_or_else(|| format!("no invite '{id}'"))?;
    if invite.status == STATUS_REVOKED {
        println!("invite {id} is already revoked");
        return Ok(true);
    }
    let previous = effective_status(&invite, Utc::now());
    invite.status = STATUS_REVOKED.to_string();
    store_invite(&store, &invite).await?;
    println!(
        "invite {id} for target '{}' is revoked (was {previous})",
        invite.target_name
    );
    println!(
        "the minted channel key is still in the credential store: stado fleet key rm '{}' removes it",
        invite.target_name
    );
    Ok(true)
}
