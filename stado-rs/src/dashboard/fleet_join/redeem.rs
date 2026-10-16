//! Redeeming one invite: verifying a presented token against its stored
//! document, spending one use as a compare-and-swap, and putting that use
//! back when the request it paid for could not be recorded.

use chrono::Utc;
use serde_json::Value;

use crate::cli::fleet::invite::{self, Invite};
use crate::queue::{JobStorage, StorageError};

use super::refusals::store_failed;

const STORE_UNAVAILABLE: &str = "enrollment store is unavailable";

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
/// is decided by `invite::effective_status`, never re-derived here. A store
/// that cannot be read, or an invite document it holds that cannot be
/// parsed, is logged with its cause for the operator.
pub(super) async fn verify(store: &JobStorage, id: &str, secret: &str) -> Result<Accepted, Denial> {
    let path = invite::invite_path(id);
    let stored = match store.read_text_versioned(&path).await {
        Ok(stored) => stored,
        Err(error) => {
            return Err(store_failed(
                STORE_UNAVAILABLE,
                &format!("read {path}"),
                &error,
            ));
        }
    };
    let found = stored.and_then(|versioned| {
        readable(&path, &versioned.content).map(|invite| Accepted {
            invite,
            version: versioned.version,
        })
    });

    // A missing invite still pays for a digest and a comparison; skipping
    // them would make "no such id" the fast answer. With no stored digest the
    // presented one is compared with itself, and the verdict below refuses
    // it because there is no invite to accept.
    let presented = invite::secret_digest(secret);
    let expected = match &found {
        Some(accepted) => accepted.invite.secret_sha256.as_str(),
        None => presented.as_str(),
    };
    let secret_ok = invite::digests_match(expected, &presented);
    let usable = found.as_ref().is_some_and(|accepted| {
        accepted.invite.id == id
            && invite::effective_status(&accepted.invite, Utc::now()) == invite::STATUS_OPEN
    });

    match found {
        Some(accepted) if secret_ok & usable => Ok(accepted),
        _ => Err(Denial::Refused),
    }
}

/// The invite a stored document holds, or `None` with the reason logged: a
/// document that is not an invite refuses every token presented for it, and
/// only the operator can repair it.
fn readable(path: &str, content: &str) -> Option<Invite> {
    let parsed = serde_json::from_str::<Value>(content)
        .map_err(|error| format!("not JSON: {error}"))
        .and_then(|document| invite::parse_invite(&document));
    match parsed {
        Ok(invite) => Some(invite),
        Err(cause) => {
            tracing::warn!(path, cause = %cause, "a stored fleet invite cannot be read, so it refuses every join");
            None
        }
    }
}

/// Consume one use, atomically against concurrent joins on the same code. A
/// lost race means the use went to another machine, which is an exhausted
/// code, which is the same refusal as any other.
pub(super) async fn spend(store: &JobStorage, accepted: &Accepted) -> Result<Value, Denial> {
    let next = invite::with_one_use_spent(&accepted.invite);
    let document = invite::invite_document(&next);
    let body = serde_json::to_string_pretty(&document)
        .map_err(|error| store_failed(STORE_UNAVAILABLE, "serialise the spent invite", &error))?;
    let path = invite::invite_path(&accepted.invite.id);
    match store
        .compare_and_swap_text(&path, &accepted.version, &body)
        .await
    {
        Ok(_) => Ok(document),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Err(Denial::Refused),
        Err(error) => Err(store_failed(
            STORE_UNAVAILABLE,
            &format!("spend a use of {path}"),
            &error,
        )),
    }
}

/// Put a spent use back when the request it paid for could not be recorded, so
/// a storage failure does not silently burn the owner's only code. If another
/// writer has since moved the document on, the use stays spent rather than
/// being resurrected under someone else's write; a refund the store refused
/// for any other reason is logged with the invite and cause, since the owner
/// then holds a code with one use fewer than they were told.
pub(super) async fn refund(store: &JobStorage, accepted: &Accepted, spent: &Value) {
    let path = invite::invite_path(&accepted.invite.id);
    let current = match store.read_text_versioned(&path).await {
        Ok(Some(current)) => current,
        Ok(None) => return,
        Err(error) => return unrefunded(accepted, &error.to_string()),
    };
    let held = match serde_json::from_str::<Value>(&current.content) {
        Ok(held) => held,
        Err(error) => {
            return unrefunded(accepted, &format!("the stored invite is not JSON: {error}"));
        }
    };
    if &held != spent {
        return;
    }
    let restored = invite::invite_document(&accepted.invite);
    let body = match serde_json::to_string_pretty(&restored) {
        Ok(body) => body,
        Err(error) => return unrefunded(accepted, &error.to_string()),
    };
    match store
        .compare_and_swap_text(&path, &current.version, &body)
        .await
    {
        Ok(_) | Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => {}
        Err(error) => unrefunded(accepted, &error.to_string()),
    }
}

fn unrefunded(accepted: &Accepted, cause: &str) {
    tracing::warn!(
        invite = %accepted.invite.id,
        cause = %cause,
        "a spent fleet invite use could not be refunded after its enrollment was not recorded"
    );
}
