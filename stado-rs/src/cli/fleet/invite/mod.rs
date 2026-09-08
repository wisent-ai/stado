//! Invite-based enrollment: `stado fleet invite|invites|revoke-invite`.
//!
//! The method exists because the previous shortest path to adding somebody
//! else's laptop was a phone call: the fleet reaches a machine over SSH with
//! the key it owns, so that key's public half had to be in the machine's
//! `authorized_keys` before `fleet enroll` could probe anything — and putting
//! it there was a human copy-paste on the far end. An invite moves that step
//! into a single line the machine's owner runs.
//!
//! What travels is a token, `<id>.<secret>`, and nothing else. The store keeps
//! only `secret_sha256`, so the object an operator (or a leak) can read cannot
//! be replayed as a credential; the secret exists in this process for exactly
//! as long as it takes to print it once, and no command can reprint it. The
//! key direction is unchanged and not negotiable: minting an invite mints the
//! fleet's own ed25519 pair through the existing `fleet key generate` path, the
//! private half stays in the operator's vault, and the machine only ever
//! receives the public half.
//!
//! Redemption is two dashboard routes authenticated by the token alone
//! (`GET /api/fleet/invite/key`, `POST /api/fleet/join`); neither may write the
//! registry. They spend the invite through [`redeem::authorize`] and
//! [`redeem::spend`] here, so the lifecycle has one implementation regardless
//! of which surface drives it.
//!
//! That is the ONLINE mode, and it needs one thing the fleet does not always
//! have: a control point the machine's owner can reach over HTTP. When there is
//! none — the name does not resolve, nothing listens, or the release serving it
//! predates the invite routes — printing the one-liner anyway would hand
//! somebody a command that cannot work, so [`invite`] probes `/join.sh` first
//! and falls back to the OFFLINE mode instead of lying.
//!
//! The offline mode carries no secret and uses no route. What travels is a
//! self-contained `sh` fragment, over whatever channel the operator is already
//! using to talk to the machine's owner, and the only key in it is the fleet's
//! PUBLIC half: intercepting the fragment gains nothing. The owner runs it, the
//! fragment installs the key and prints the `user@address` to send back, and the
//! operator closes the invite with the ordinary
//! `fleet enroll NAME --ssh ADDRESS --bootstrap` — which reaches
//! [`close_offline_for_target`] and spends the invite through the same
//! [`mark_spent`] that `approve` uses. No second state machine.
//!
//! The seams are the components: `record` is the stored object and its
//! persistence, `mint` mints, `redeem` spends and retires, and `checkpoint`
//! is what the control point turned out to be.

// Public because the re-exported `probe_checkpoint` returns `Checkpoint`: a
// public function may not hand back a type callers cannot name.
pub mod checkpoint;
mod mint;
mod record;
// Public because `authorize` and `spend` are the redemption surface the
// dashboard's join routes are documented against; nothing inside this tree
// calls them.
pub mod redeem;

pub use checkpoint::probe_checkpoint;
pub use mint::command::invite;
pub use record::listing::invites;
pub use record::token::{digests_match, parse_token};
pub use record::{
    effective_status, invite_document, invite_path, parse_invite, secret_digest, Invite,
    STATUS_OPEN, STATUS_SPENT,
};
pub use redeem::mark_spent;
pub use redeem::offline_close::close_offline_for_target;
pub use redeem::revoke::revoke_invite;

/// Bytes of invite identity and of invite secret. The id is public and only
/// has to be unique; the secret is the credential.
const ID_BYTES: usize = 8;
const SECRET_BYTES: usize = 32;

/// One refusal for every unusable token. A caller learning *why* a token was
/// refused learns whether an id exists, whether it was already used and when
/// it lapsed — three answers an unauthenticated redeemer has no business
/// getting.
const REFUSED: &str = "invite token is not usable";
