//! What the store said about one object, and which unanswered state a
//! refusal is.

use crate::cli::storage::*;

/// What the store said about one object.
///
/// Five states, not three. Three collapsed every way of not getting an answer
/// into `unreachable`, so a `401` refusal, a `503` boundary that is down and
/// the resolver's own `502 upstream unavailable` all arrived as one verdict,
/// separable only by reading a detail string -- and a caller asking "is this
/// coordinate spent" cannot branch on prose. Two releases turned on that
/// question on 2026-09-03 and got `unreachable` for three different causes
/// with three different remedies. Each of these is something a reader can act
/// on: fix a credential for the refused, retry the unavailable, chase the
/// transport for the unreachable.
pub(in crate::cli::storage) enum Presence {
    /// The store answered and the object is there.
    Present {
        size: usize,
        /// Backend generation / ETag, when the versioned read produced one.
        version: Option<String>,
        /// Why the version is missing, when it is.
        detail: Option<String>,
    },
    /// The store answered and the object is NOT there.
    Absent,
    /// The store answered and refused the question: this reader may not ask
    /// it. Nothing is known about the object, and asking again unchanged
    /// cannot learn anything.
    Refused(String),
    /// The store answered that it cannot answer right now. Nothing is known
    /// about the object, and the same question may be answered later.
    Unavailable(String),
    /// Nothing answered at all. This is the state `BlobBackend::exists`
    /// cannot express.
    Unreachable(String),
}

impl Presence {
    /// The one-word verdict a script branches on. The exit code says only
    /// whether the question was answered, so the three unanswered states have
    /// to be distinguishable here or they are not distinguishable at all.
    pub(in crate::cli::storage) fn state(&self) -> &'static str {
        match self {
            Self::Present { .. } => "present",
            Self::Absent => "absent",
            Self::Refused(_) => "refused",
            Self::Unavailable(_) => "unavailable",
            Self::Unreachable(_) => "unreachable",
        }
    }

    /// Whether the store answered the question that was asked.
    ///
    /// `present` and `absent` are answers; the other three are not. The
    /// exit-code contract turns on exactly this, so a caller can never read a
    /// store that did not answer as a store that answered "gone".
    pub(in crate::cli::storage) fn answered(&self) -> bool {
        matches!(self, Self::Present { .. } | Self::Absent)
    }

    pub(in crate::cli::storage) fn detail(&self) -> Option<String> {
        match self {
            Self::Present { detail, .. } => detail.clone(),
            Self::Absent => None,
            Self::Refused(detail) | Self::Unavailable(detail) | Self::Unreachable(detail) => {
                Some(detail.clone())
            }
        }
    }

    /// Why this unanswered question stays unanswered, naming what the caller
    /// can do about THIS state rather than about not-answering in general.
    ///
    /// Empty for an answered question, which needs no such sentence. Every
    /// caller reaches this only behind [`Presence::answered`], so that one
    /// predicate stays the single statement of the exit-code contract and
    /// this stays the single statement of the reason.
    pub(in crate::cli::storage) fn unanswered_sentence(&self, path: &str) -> String {
        let (verdict, remedy, detail) = match self {
            Self::Present { .. } | Self::Absent => return String::new(),
            Self::Refused(detail) => (
                "REFUSED",
                "the store answered that this reader may not ask: repair the credential or the \
                 grant, because the same question asked again cannot learn anything",
                detail,
            ),
            Self::Unavailable(detail) => (
                "UNAVAILABLE",
                "the store answered that it cannot answer right now: this same question may be \
                 answered later, so retry it",
                detail,
            ),
            Self::Unreachable(detail) => (
                "UNREACHABLE",
                "nothing answered at all: chase the transport in front of the store",
                detail,
            ),
        };
        format!(
            "{path:?} is {verdict}, not absent — {remedy}: {detail}. Treat the object's \
             existence as unknown."
        )
    }
}

/// Which unanswered state one HTTP status is.
///
/// Only ever reached for a status that is not a success, not a redirect and
/// not a 404, so every arm here is a way of not answering the question.
/// `401`/`403` is the store refusing it. `429`/`503` is the store saying "not
/// now" -- the object plane answers `503 object authorization unavailable`
/// when its Skarbiec boundary is down, which is a wait, not a dead store. A
/// gateway status is the proxy in front of the store rather than the store:
/// Stado's own service resolver writes exactly `502 upstream unavailable`
/// when its SSH forward cannot carry a connection, and nothing answered in
/// that case, so it has to stay `unreachable`.
pub(in crate::cli::storage) fn unanswered_for_status(status: u16, detail: String) -> Presence {
    match status {
        401 | 403 => Presence::Refused(detail),
        429 | 503 => Presence::Unavailable(detail),
        _ => Presence::Unreachable(detail),
    }
}

/// The same judgement for a backend error, which carries a status when the
/// backend spoke HTTP and carries none when the failure was below HTTP.
pub(in crate::cli::storage) fn unanswered_for_error(error: &StorageError) -> Presence {
    let detail = error.to_string();
    match error {
        StorageError::Stado { status, .. } | StorageError::Gcs { status, .. } => {
            unanswered_for_status(*status, detail)
        }
        // Authentication that could not be established is this reader lacking
        // standing to ask, which is a refusal however far below HTTP it
        // happened.
        StorageError::Auth(_) => Presence::Refused(detail),
        _ => Presence::Unreachable(detail),
    }
}
