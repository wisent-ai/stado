//! What one checkout owes the delivery loop, read from its `release newest`
//! plan: a release, a record of one already published, nothing yet because a
//! run is releasing it, or a refusal naming why its version cannot advance.

use crate::cli::release_newest::Standing;

/// What one checkout owes, read from its plan.
pub(super) enum Owed {
    /// Release `commit` under `version`, after committing `version` first
    /// when `bump` says the declared one is taken by another commit's run.
    Release {
        commit: String,
        version: String,
        bump: bool,
    },
    /// Published already: only the record in Oko is owed.
    Record { commit: String, version: String },
    /// A run is releasing `commit` now; the next pass reads its outcome.
    InFlight { commit: String, version: String },
    /// The declared version is taken and cannot advance: `error` says why.
    Unadvanceable { commit: String, error: String },
    /// The product declares it does not release, or its checkout could not
    /// be read; `release newest` names either in its own listing.
    Nothing,
}

pub(super) fn owed(standing: Standing) -> Owed {
    match standing {
        Standing::Releasable {
            commit, version, ..
        } => Owed::Release {
            commit,
            version,
            bump: false,
        },
        Standing::VersionTaken {
            commit,
            version,
            next,
            unadvanceable,
            ..
        } => match (next, unadvanceable) {
            (Some(next), _) => Owed::Release {
                commit,
                version: next,
                bump: true,
            },
            (None, Some(why)) => Owed::Unadvanceable {
                commit,
                error: format!(
                    "{version} is taken by another commit's release and the versioning rule gives no next version: {why}"
                ),
            },
            (None, None) => Owed::Unadvanceable {
                commit,
                error: format!(
                    "{version} is taken by another commit's release and the plan named neither the next version nor why there is none"
                ),
            },
        },
        Standing::Published {
            commit, version, ..
        } => Owed::Record { commit, version },
        Standing::InFlight {
            commit, version, ..
        } => Owed::InFlight { commit, version },
        Standing::Unreadable { .. } | Standing::DeclaresNoReleases { .. } => Owed::Nothing,
    }
}
