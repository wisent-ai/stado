//! Redeeming one invite: verifying a presented token against its stored
//! document, spending one use as a compare-and-swap, and putting that use
//! back when the request it paid for could not be recorded.

use chrono::Utc;
use serde_json::Value;

use crate::cli::fleet::invite::{self, Invite};
use crate::queue::{JobStorage, StorageError};

/// Compared against when no digest could be loaded, so an unknown id costs
/// the same hash and the same comparison as a known one.
const ABSENT_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

// ---------------------------------------------------------------------------
// Invite verification
// ---------------------------------------------------------------------------

/// Why a route stopped. `Refused` is the single indistinguishable answer;
/// `Unavailable` is infrastructure, which says nothing about the token.
pub(super) enum Denial {
    Refused,
    Unavailable(&'static str),
}

/// An invite that verified, with the version its document was read at so the
/// spend can be a compare-and-swap.
pub(super) struct Accepted {
    pub(super) invite: Invite,
    version: String,
}

/// Verify a presented token against its stored invite.
///
/// Every state — unknown id, wrong secret, unparsable document, revoked,
/// spent, exhausted, expired — collapses into `Denial::Refused` after the
/// same work: one store read, one digest, one constant-time comparison. There
/// is no early return between the read and the verdict, and the status itself
/// is decided by `invite::effective_status`, never re-derived here.
pub(super) async fn verify(store: &JobStorage, id: &str, secret: &str) -> Result<Accepted, Denial> {
    let stored = match store.read_text_versioned(&invite::invite_path(id)).await {
        Ok(stored) => stored,
        Err(_) => return Err(Denial::Unavailable("enrollment store is unavailable")),
    };
    let (content, version) = match stored {
        Some(versioned) => (versioned.content, versioned.version),
        None => (String::new(), String::new()),
    };
    let parsed = serde_json::from_str::<Value>(&content)
        .ok()
        .and_then(|document| invite::parse_invite(&document).ok());

    // A missing invite still pays for a digest and a comparison; skipping
    // them would make "no such id" the fast answer.
    let expected = parsed
        .as_ref()
        .map(|invite| invite.secret_sha256.as_str())
        .unwrap_or(ABSENT_DIGEST);
    let secret_ok = invite::digests_match(expected, &invite::secret_digest(secret));
    let usable = parsed.as_ref().is_some_and(|invite| {
        invite.id == id && invite::effective_status(invite, Utc::now()) == invite::STATUS_OPEN
    });

    match parsed {
        Some(invite) if secret_ok & usable => Ok(Accepted { invite, version }),
        _ => Err(Denial::Refused),
    }
}

/// Consume one use, atomically against concurrent joins on the same code. A
/// lost race means the use went to another machine, which is an exhausted
/// code, which is the same refusal as any other.
pub(super) async fn spend(store: &JobStorage, accepted: &Accepted) -> Result<Value, Denial> {
    let mut next = accepted.invite.clone();
    next.uses_spent = next.uses_spent.saturating_add(u64::from(true));
    if next.uses_spent >= next.uses_allowed {
        next.status = invite::STATUS_SPENT.to_string();
    }
    let document = invite::invite_document(&next);
    let body = serde_json::to_string_pretty(&document)
        .map_err(|_| Denial::Unavailable("enrollment store is unavailable"))?;
    match store
        .compare_and_swap_text(
            &invite::invite_path(&accepted.invite.id),
            &accepted.version,
            &body,
        )
        .await
    {
        Ok(_) => Ok(document),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Err(Denial::Refused),
        Err(_) => Err(Denial::Unavailable("enrollment store is unavailable")),
    }
}

/// Put a spent use back when the request it paid for could not be recorded, so
/// a storage failure does not silently burn the owner's only code. Best
/// effort: if another writer has since moved the document on, the use stays
/// spent rather than being resurrected under someone else's write.
pub(super) async fn refund(store: &JobStorage, accepted: &Accepted, spent: &Value) {
    let path = invite::invite_path(&accepted.invite.id);
    let Ok(Some(current)) = store.read_text_versioned(&path).await else {
        return;
    };
    if serde_json::from_str::<Value>(&current.content)
        .ok()
        .as_ref()
        != Some(spent)
    {
        return;
    }
    let restored = invite::invite_document(&accepted.invite);
    let Ok(body) = serde_json::to_string_pretty(&restored) else {
        return;
    };
    let _ = store
        .compare_and_swap_text(&path, &current.version, &body)
        .await;
}
