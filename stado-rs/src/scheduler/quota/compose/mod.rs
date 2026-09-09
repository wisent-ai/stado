//! Everything downstream of a live limit map: `overlay` reads the
//! storage-backed reservation file and composes it with the live limits
//! into the per-provider quota dict, `available` turns that dict plus the
//! provider's running instances into the headroom the dispatcher admits
//! against, and `summary` renders the same arithmetic across every
//! configured provider for the operator-facing views.

pub(super) mod available;
pub(super) mod overlay;
pub(super) mod summary;
