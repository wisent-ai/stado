//! The two halves a freshness verdict is decided from: the vocabulary
//! `skarbiec totp-seed-state` answers the vault's half in, and one reduced
//! sign-in attempt from the run history's half.

/// What the vault said about one row, as `skarbiec totp-seed-state` spells it.
pub const SEED_PRESENT: &str = "present";
pub const SEED_DECLARED_EMPTY: &str = "declared_empty";
pub const SEED_FIELD_ABSENT: &str = "field_absent";
pub const SEED_UNREADABLE: &str = "unreadable";
/// The host's Skarbiec build has no `totp-seed-state`, so the vault's half is
/// unavailable there. Reported as its own condition rather than guessed at:
/// the run history's half is still evidence, and a diagnostic that answers
/// nothing because one source is missing is the failure this command exists to
/// correct.
pub const SEED_READ_UNSUPPORTED: &str = "vault_read_unsupported";

/// One sign-in attempt, reduced to the facts a freshness verdict turns on.
///
/// `code_submitted` is the load-bearing one. An attempt that never reached the
/// authenticator step proves nothing about the seed, and counting it as a
/// rejection is how "the provider is down" would get misreported as "the seed
/// is stale" — a different condition with a different repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub at: String,
    pub at_ms: i64,
    pub result: String,
    /// A code computed from the stored seed was typed into the challenge.
    pub code_submitted: bool,
    /// The provider refused that code, in its own words or after retries.
    pub code_rejected: bool,
    /// Google answered "Too many failed attempts" — the authenticator method
    /// is locked, which is a consequence of resubmitting a stale seed and
    /// blocks the operator's own repair until it clears.
    pub locked_out: bool,
    /// The authenticator step was never usable, so this attempt is silent
    /// about the seed.
    pub authenticator_unreached: bool,
    /// The markers the host matched, for an operator who wants the trail.
    pub markers: Vec<String>,
}
