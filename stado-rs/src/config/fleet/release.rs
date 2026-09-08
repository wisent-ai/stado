//! Deployment endpoints and the immutable release the fleet runs.

use crate::config_file::resolve as cfg;

/// Canonical Stado API origin used by object and immutable-release clients.
/// `api.url` is the deployment endpoint; releases do not own a second origin.
pub fn stado_api_url() -> String {
    cfg("STADO_API_URL", "api.url", "")
        .trim_end_matches('/')
        .to_string()
}

/// Public origin that serves the three enrollment routes (`GET /join.sh`,
/// `GET /api/fleet/invite/key`, `POST /api/fleet/join`) — env
/// `STADO_ENROLLMENT_URL`, config key `enrollment.url`, empty by default.
///
/// This is deliberately NOT [`stado_api_url`]. `api.url` is the deployment
/// endpoint that self-update, remote bootstrap, cloud-agent dispatch and the
/// coordinator resolve their release channel from; pointing it at a narrow
/// enrollment listener would break all of those. A publicly tunnelled
/// `stado dashboard --enrollment-only` listener serves only enrollment, so it
/// needs its own origin. Empty means "no separate enrollment origin", and
/// every caller falls back to `api.url` exactly as before.
pub fn enrollment_url() -> String {
    cfg("STADO_ENROLLMENT_URL", "enrollment.url", "")
        .trim_end_matches('/')
        .to_string()
}

/// Exact immutable Stado version consumed by bootstrap and cloud agents (env
/// `STADO_RELEASE_VERSION`, config key `release.version`).
pub fn stado_release_version() -> String {
    cfg("STADO_RELEASE_VERSION", "release.version", "")
        .trim()
        .to_string()
}

/// Exact release platform shipped to cloud-agent templates (env
/// `STADO_RELEASE_PLATFORM`, config key `release.platform`). Remote bootstrap
/// derives its exact platform from the remote kernel and architecture.
pub fn stado_release_platform() -> String {
    cfg("STADO_RELEASE_PLATFORM", "release.platform", "")
        .trim()
        .to_string()
}

/// Skarbiec key-pair item containing the base64 Ed25519 PKCS#8 release
/// authority key in `private_key`. The item name is configuration; key bytes
/// never enter a product manifest or registry document.
pub fn release_signing_key_item() -> String {
    cfg(
        "STADO_RELEASE_SIGNING_KEY_ITEM",
        "release.signing_key_item",
        "stado-release-signing",
    )
    .trim()
    .to_string()
}

/// Trusted release-control key identifier paired with
/// [`release_signing_key_item`].
pub fn release_signing_key_id() -> String {
    cfg(
        "STADO_RELEASE_SIGNING_KEY_ID",
        "release.signing_key_id",
        "stado-release-2026-08",
    )
    .trim()
    .to_string()
}

/// Exact immutable release object containing the cloud-agent Python
/// environment and model cache. There is deliberately no default: dispatch
/// refuses to create a machine until the operator publishes and selects one.
pub fn stado_agent_runtime_bundle_uri() -> String {
    cfg(
        "STADO_AGENT_RUNTIME_BUNDLE_URI",
        "release.agent_runtime_bundle_uri",
        "",
    )
    .trim()
    .to_string()
}

/// SHA-256 of [`stado_agent_runtime_bundle_uri`], checked before extraction.
pub fn stado_agent_runtime_bundle_sha256() -> String {
    cfg(
        "STADO_AGENT_RUNTIME_BUNDLE_SHA256",
        "release.agent_runtime_bundle_sha256",
        "",
    )
    .trim()
    .to_string()
}
