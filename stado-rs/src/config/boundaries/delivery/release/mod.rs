//! Release publication constants.

mod endpoints;
mod publishers;

pub use endpoints::*;
pub use publishers::*;

/// Active authenticated software publishers. Public readers use the separate
/// tokenless release GET route.
pub const ACTIVE_RELEASE_PUBLISHERS: &[&str] = &[
    "brama",
    "compute-marketplace",
    "image-video-router",
    "oko",
    "skarbiec",
    "stado",
    "trading-autonomy",
    "wisent-backend",
];

pub const RELEASE_API_VERIFIER_CONSUMER: &str = "stado-release-api-verifier";

/// The consumer the vault already authorizes to read the release authority's
/// private key, and nothing else: its single minted capability is
/// `read:stado-release-signing#private_key`.
///
/// `release submit` read that key through `secrets.skarbiec.consumer`, the broad
/// control-plane grant, which the vault correctly refuses. The refusal arrived as
/// a bare `403 consumer not authorized to read item field` naming neither the
/// consumer it wanted nor the one it got, and the vault's own policy had the
/// answer the whole time.
pub const RELEASE_SIGNING_CONSUMER: &str = "stado-release-coordinator";
