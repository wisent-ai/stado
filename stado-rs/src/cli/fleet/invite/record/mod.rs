//! The invite record: where it lives, the markers it carries, and the two
//! pure conversions between the stored document and the in-process struct.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::primitives::tally::Tally;

pub(in crate::cli::fleet::invite) mod listing;
pub(in crate::cli::fleet::invite) mod store;
pub(in crate::cli::fleet::invite) mod token;

/// Store prefix every invite object lives under. It sits beneath the join
/// requests' own prefix, so `fleet pending` (which lists `enrollments/`) must
/// keep ignoring documents it cannot parse as a request — it already does.
pub const INVITES_PREFIX: &str = "enrollments/invites/";

/// Invite lifecycle markers. `open` and `revoked` are stored; `spent` is both
/// stored and derived from the use counter; `expired` is only ever derived, so
/// an invite going stale needs no writer to notice it.
pub const STATUS_OPEN: &str = "open";
pub const STATUS_SPENT: &str = "spent";
pub const STATUS_REVOKED: &str = "revoked";
pub const STATUS_EXPIRED: &str = "expired";

/// How an invite is redeemed. `online` is the token-and-route mode; `offline`
/// is the pasted-fragment mode, which has no secret to present and no route to
/// present it to. An object written before the modes existed has no `mode`
/// field and is read as `online`, which is what it was.
pub const MODE_ONLINE: &str = "online";
pub const MODE_OFFLINE: &str = "offline";

/// A stored invite. `secret_sha256` is the only trace of the secret anywhere,
/// and it is absent for exactly one reason: an offline invite never had a
/// secret, so no presented token can be compared with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub id: String,
    pub secret_sha256: Option<String>,
    pub target_name: String,
    pub created_at: String,
    pub expires_at: String,
    pub uses_allowed: u64,
    pub uses_spent: u64,
    pub status: String,
    pub created_by: String,
    pub mode: String,
}

/// Store path of one invite.
pub fn invite_path(id: &str) -> String {
    format!("{INVITES_PREFIX}{id}.json")
}

/// Hex SHA-256 of a secret — the only form the store ever sees.
pub fn secret_digest(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parse a stored invite document. Pure.
///
/// `secret_sha256` is required of an online invite and refused of an offline
/// one: a stored offline object carrying a digest would mean something minted
/// a secret for a mode that has nothing to present it to. Every other member
/// is required; one that is missing is refused by name, never read as a
/// count or a writer nobody recorded.
pub fn parse_invite(document: &Value) -> Result<Invite, String> {
    let field = |name: &str| -> Result<String, String> {
        document
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("invite object has no '{name}'"))
    };
    let counter = |name: &str| -> Result<u64, String> {
        document
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("invite object has no whole-number '{name}'"))
    };
    let mode = match document.get("mode").and_then(Value::as_str) {
        None | Some(MODE_ONLINE) => MODE_ONLINE.to_string(),
        Some(MODE_OFFLINE) => MODE_OFFLINE.to_string(),
        Some(other) => return Err(format!("invite object has an unknown mode '{other}'")),
    };
    let secret_sha256 = if mode == MODE_OFFLINE {
        if document.get("secret_sha256").is_some() {
            return Err("an offline invite object must carry no 'secret_sha256'".to_string());
        }
        None
    } else {
        Some(field("secret_sha256")?)
    };
    Ok(Invite {
        id: field("id")?,
        secret_sha256,
        target_name: field("target_name")?,
        created_at: field("created_at")?,
        expires_at: field("expires_at")?,
        uses_allowed: counter("uses_allowed")?,
        uses_spent: counter("uses_spent")?,
        status: field("status")?,
        created_by: field("created_by")?,
        mode,
    })
}

/// Render an invite as its stored document. Pure; carries no secret. An
/// offline invite has no digest to write, so the key is absent rather than
/// present and empty — the store never holds a field that reads like a
/// credential nobody can use.
pub fn invite_document(invite: &Invite) -> Value {
    let mut document = json!({
        "id": invite.id,
        "target_name": invite.target_name,
        "created_at": invite.created_at,
        "expires_at": invite.expires_at,
        "uses_allowed": invite.uses_allowed,
        "uses_spent": invite.uses_spent,
        "status": invite.status,
        "created_by": invite.created_by,
        "mode": invite.mode,
    });
    if let Some(digest) = &invite.secret_sha256 {
        document["secret_sha256"] = Value::String(digest.clone());
    }
    document
}

/// The status an invite actually has now, which is not always the status on
/// disk: a revocation and an exhausted counter are recorded, a lapsed deadline
/// is not. Ordering is by permanence — revoked, then spent, then expired —
/// because a token that was used and then lapsed is more usefully reported as
/// spent. Pure.
pub fn effective_status(invite: &Invite, now: DateTime<Utc>) -> &'static str {
    if invite.status == STATUS_REVOKED {
        return STATUS_REVOKED;
    }
    if invite.status == STATUS_SPENT || invite.uses_spent >= invite.uses_allowed {
        return STATUS_SPENT;
    }
    // An expiry still ahead keeps the invite open. An unparsable expiry is
    // not a permit: reading it as "never expires" would turn one corrupt
    // field into an eternal credential, so it reads as expired.
    match DateTime::parse_from_rfc3339(&invite.expires_at) {
        Ok(expires) if now < expires.with_timezone(&Utc) => STATUS_OPEN,
        _ => STATUS_EXPIRED,
    }
}

/// The invite after one more redemption, closed when the allowance runs out.
/// The CLI's redemption route and the dashboard's join route both spend
/// through this, so a use is counted one way.
pub fn with_one_use_spent(invite: &Invite) -> Invite {
    let mut spent = invite.clone();
    spent.uses_spent.tally();
    if spent.uses_spent >= spent.uses_allowed {
        spent.status = STATUS_SPENT.to_string();
    }
    spent
}
