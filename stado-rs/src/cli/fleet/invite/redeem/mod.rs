//! Redemption: resolve a presented token to the invite it may spend, count
//! the use, and close the invite that has produced a registered machine.

use chrono::Utc;

use crate::cli::fleet::invite::record::store::{load_invite, store_invite};
use crate::cli::fleet::invite::record::token::{digests_match, parse_token};
use crate::cli::fleet::invite::record::{
    effective_status, secret_digest, Invite, MODE_OFFLINE, STATUS_OPEN, STATUS_SPENT,
};
use crate::cli::fleet::invite::REFUSED;
use crate::queue::JobStorage;

pub(in crate::cli::fleet::invite) mod offline_close;
pub(in crate::cli::fleet::invite) mod revoke;

/// Resolve a presented token to the invite it may spend, or refuse without
/// saying which of unknown/spent/revoked/expired applies.
///
/// An offline invite is refused here by mode as well as by digest. Its stored
/// digest is empty, so no token could match it anyway; saying so explicitly
/// means a future writer that puts a digest on an offline object still cannot
/// turn a pasted fragment into a redeemable credential.
pub async fn authorize(store: &JobStorage, token: &str) -> Result<Invite, String> {
    let (id, secret) = parse_token(token)?;
    let invite = load_invite(store, id).await?.ok_or(REFUSED)?;
    if invite.mode == MODE_OFFLINE {
        return Err(REFUSED.to_string());
    }
    if !digests_match(&invite.secret_sha256, &secret_digest(secret)) {
        return Err(REFUSED.to_string());
    }
    if effective_status(&invite, Utc::now()) != STATUS_OPEN {
        return Err(REFUSED.to_string());
    }
    Ok(invite)
}

/// Count one redemption against an invite, closing it when the allowance runs
/// out. Called by the redemption route after the request it authorized has been
/// filed.
pub async fn spend(store: &JobStorage, invite: &Invite) -> Result<Invite, String> {
    let mut spent = invite.clone();
    spent.uses_spent = spent.uses_spent.saturating_add(1);
    if spent.uses_spent >= spent.uses_allowed {
        spent.status = STATUS_SPENT.to_string();
    }
    store_invite(store, &spent).await?;
    Ok(spent)
}

/// Close an invite that has produced a registered target: approval is the end
/// of its life regardless of any allowance left over.
pub async fn mark_spent(store: &JobStorage, id: &str) -> Result<(), String> {
    let Some(mut invite) = load_invite(store, id).await? else {
        return Ok(());
    };
    if invite.status == STATUS_SPENT {
        return Ok(());
    }
    invite.status = STATUS_SPENT.to_string();
    store_invite(store, &invite).await
}
