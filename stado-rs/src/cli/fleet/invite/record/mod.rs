//! The invite record: where it lives, the markers it carries, and the two
//! pure conversions between the stored document and the in-process struct.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

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
/// and it is empty for exactly one reason: an offline invite never had a
/// secret. An empty digest is not a weak digest — no input hashes to it, so a
/// presented token cannot match one, which is the same refusal an unknown id
/// gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub id: String,
    pub secret_sha256: String,
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
/// a secret for a mode that has nothing to present it to.
pub fn parse_invite(document: &Value) -> Result<Invite, String> {
    let field = |name: &str| -> Result<String, String> {
        document
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("invite object has no '{name}'"))
    };
    let counter = |name: &str, when_absent: u64| -> u64 {
        document
            .get(name)
            .and_then(Value::as_u64)
            .unwrap_or(when_absent)
    };
    let mode = match document.get("mode").and_then(Value::as_str) {
        None | Some("") | Some(MODE_ONLINE) => MODE_ONLINE.to_string(),
        Some(MODE_OFFLINE) => MODE_OFFLINE.to_string(),
        Some(other) => return Err(format!("invite object has an unknown mode '{other}'")),
    };
    let secret_sha256 = if mode == MODE_OFFLINE {
        if document.get("secret_sha256").is_some() {
            return Err("an offline invite object must carry no 'secret_sha256'".to_string());
        }
        String::new()
    } else {
        field("secret_sha256")?
    };
    Ok(Invite {
        id: field("id")?,
        secret_sha256,
        target_name: field("target_name")?,
        created_at: field("created_at")?,
        expires_at: field("expires_at")?,
        uses_allowed: counter("uses_allowed", 1),
        uses_spent: counter("uses_spent", 0),
        status: field("status")?,
        created_by: document
            .get("created_by")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
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
    if !invite.secret_sha256.is_empty() {
        document["secret_sha256"] = Value::String(invite.secret_sha256.clone());
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
    match DateTime::parse_from_rfc3339(&invite.expires_at) {
        Ok(deadline) if now >= deadline.with_timezone(&Utc) => STATUS_EXPIRED,
        // An unparsable deadline is not a permit. Treating it as "never
        // expires" would turn one corrupt field into an eternal credential.
        Err(_) => STATUS_EXPIRED,
        Ok(_) => STATUS_OPEN,
    }
}
