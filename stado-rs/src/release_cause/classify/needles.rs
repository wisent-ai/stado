//! The sentences each cause is recognised by, one list per cause.
//!
//! Every phrase is lower-cased here because the haystack is lower-cased once,
//! in the classifier, before any of these are looked for.

/// The candidate declared no rollback compatibility. The agent's own sentence,
/// from [`crate::release_agent`], and therefore the strongest evidence there
/// is: it is not a report copied out of a log that may have been truncated.
pub(super) const ROLLBACK_COMPATIBILITY_NEEDLES: &[&str] =
    &["does not declare rollback compatibility with"];

/// The store could not be opened at all. `spawn gpg` is here because that is
/// how the one recorded instance reads: the decrypt helper was not installed
/// on the host, and no route repair addresses that.
pub(super) const CREDENTIAL_STORE_NEEDLES: &[&str] = &["cannot be decrypted", "spawn gpg"];

/// A routed coordinate that cannot serve a value.
///
/// The first three are the sentences the vault and its consumers print about
/// the whole class — the gateway's redemption wording, the vault doctor's
/// summary, and the refusal remedy `capability-issue` now prints for any
/// coordinate problem. The rest are the individual coordinate verdicts, so a
/// record that carries only the specific sentence still classifies.
pub(super) const CREDENTIAL_CANNOT_SERVE_NEEDLES: &[&str] = &[
    "no value at",
    "cannot serve a credential",
    "inspect every route with",
    "is present but empty",
    "is not a text value",
    "does not open:",
    "is in trash",
    "was renamed to",
    "no vault item",
];

/// Nothing maps the resource onto a coordinate. The first two are the
/// gateway's account of an empty or missing routes table; the third is the
/// refusal `capability-issue` prints when one resource resolves to nothing.
pub(super) const CAPABILITY_ROUTES_NEEDLES: &[&str] = &[
    "no capability was issued for any provider",
    "routes table is missing or maps nothing",
    "no capability route maps",
];

/// A capability that existed and was refused when it was spent.
pub(super) const CAPABILITY_REDEMPTION_NEEDLES: &[&str] = &[
    "redemption denied",
    "refused to redeem",
    "did not redeem",
    "capability_redeem_refused",
    "credential_redeem_failed",
    "capability is not issued",
];

/// The probe got no answer at all. One sentence only: `answered HTTP 503` is
/// a candidate saying it is unhealthy, `refused the connection` is a process
/// that is gone, and the bare `did not become ready before deadline` names
/// nothing at all — each of those has its own reading. Looked for last, so a
/// credential or capability sentence in the retained stderr still wins: the
/// timeout is the outermost symptom, never the deepest cause.
pub(super) const READINESS_UNANSWERED_NEEDLES: &[&str] = &["did not answer within"];

/// Another process already holds the port the release must serve on. Both
/// spellings are real: the operating system's own `Address already in use`,
/// which every Rust binder prints through `std::io::Error`, and the errno
/// name Node and Go processes print instead. Looked for before the readiness
/// timeout, because a candidate that could not bind is the reason the probe
/// went unanswered, never the other way round.
pub(super) const STABLE_BIND_OCCUPIED_NEEDLES: &[&str] = &["address already in use", "eaddrinuse"];
