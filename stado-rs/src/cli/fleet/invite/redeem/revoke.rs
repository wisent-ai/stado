//! The operator's refusal: retire an invite so no redemption of it can
//! succeed, and say what it was before.

use chrono::Utc;

use crate::cli::fleet::invite::record::store::{load_invite, store_invite};
use crate::cli::fleet::invite::record::{effective_status, STATUS_REVOKED};
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::queue::JobStorage;

/// `stado fleet revoke-invite ID` — retire an invite before anybody uses it.
/// Sentences, or with `--json` `{invite, target, previous, revoked,
/// channel_key_item_remains}`.
pub async fn revoke_invite(id: &str, as_json: bool) -> Result<bool, CmdError> {
    let store = JobStorage::new().await?;
    let mut invite = load_invite(&store, id).await?.ok_or_else(|| {
        CmdError::click(format!("no invite '{id}'")).stating(FailureCode::NotFound)
    })?;
    let already = invite.status == STATUS_REVOKED;
    let previous = effective_status(&invite, Utc::now());
    if !already {
        invite.status = STATUS_REVOKED.to_string();
        store_invite(&store, &invite).await?;
    }
    if as_json {
        let answer = serde_json::json!({
            "invite": id,
            "target": invite.target_name,
            "previous": previous,
            "revoked": !already,
            "channel_key_item_remains": true,
        });
        crate::cli::print_answer(&answer, true)?;
        return Ok(true);
    }
    if already {
        println!("invite {id} is already revoked");
        return Ok(true);
    }
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
