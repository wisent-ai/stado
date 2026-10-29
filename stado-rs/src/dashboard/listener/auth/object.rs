//! Object-plane bearers: the namespace grant one product object authorizes
//! against, the immutable release publisher one release coordinate authorizes
//! against, the host beacon publisher, and which of the four faults refused.

use crate::config;
use crate::dashboard::listener::http::Request;
use crate::dashboard::listener::Dashboard;

use super::{constant_time_eq, presented_bearer, AuthorityUnavailable};

/// Accept a bearer only after the request has resolved to one canonical
/// namespace and key boundary. Out-of-scope requests and bearer mismatches are
/// unauthorized; invalid configuration or an unavailable exact item is
/// reported separately so the route can return a redacted 503.
pub(crate) fn release_object_namespace(namespace: &str) -> bool {
    crate::remote::object_store::release_namespace(namespace)
}

/// Return the immutable target governed by one disposable chunk key.
///
/// Release objects remain undeletable. Only a staging shape the chunked
/// uploader has emitted (its current part names and the earlier ones,
/// `queue::is_staged_part_name`) can be removed, and it is authenticated
/// against the final target rather than against an independently chosen
/// prefix.
pub(crate) fn release_upload_target_key(key: &str) -> Option<&str> {
    let (target, suffix) = key.split_once(crate::queue::UPLOAD_PART_MARKER)?;
    let mut parts = suffix.split('/');
    let upload_id = parts.next()?;
    let chunk_index = parts.next()?;
    // The upload id is the hex of a SHA-256, so its length is the digest's
    // byte count doubled; the chunk index is the part name its writer gave it.
    let digest_bytes = <sha2::Sha256 as sha2::Digest>::output_size();
    if target.is_empty()
        || upload_id.len() != digest_bytes + digest_bytes
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || !crate::queue::is_staged_part_name(chunk_index)
        || parts.next().is_some()
    {
        return None;
    }
    Some(target)
}

/// Route-scoped host beacon publication: the `token` of the item that plays
/// role `host-health-api` and nothing else. The dashboard resolves it
/// through the same dedicated verifier grant as its object routes, never
/// through the broad coordinator credential. Machine publishers are
/// authorized separately through their exact client policies.
///
/// `Ok(false)` is a rejected bearer. `Err` is this service being unable to
/// read the item it compares against — a local, retryable fault, carrying
/// its cause, that the caller cannot fix by presenting a different
/// credential.
pub(crate) async fn authorize_host_health(
    dashboard: &Dashboard,
    request: &Request,
) -> Result<bool, AuthorityUnavailable> {
    let Some(supplied) = presented_bearer(request) else {
        return Ok(false);
    };
    let expected = dashboard.host_health_token().await?;
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

/// Authorize one object request against its declared namespace and distinguish
/// a missing declaration, an uncovered key, an absent bearer, and a mismatch.
/// Each refusal requires a different repair.
pub(crate) async fn authorize_object(
    dashboard: &Dashboard,
    request: &Request,
    namespace: &str,
    key_or_prefix: &str,
    list: bool,
    action: &str,
) -> ObjectDecision {
    let namespaces = config::object_api_namespaces().map_err(|refusals| {
        AuthorityUnavailable::new(format!(
            "object_api.namespaces is refused: {}",
            refusals.join("; ")
        ))
    })?;
    let Some(policy) = namespaces.get(namespace) else {
        return Ok(Some("no_namespace_declared"));
    };
    let in_scope = if list {
        policy
            .authorized_list_prefix(key_or_prefix, action)
            .is_some()
    } else {
        policy.allows_object_action(key_or_prefix, action)
    };
    if !in_scope {
        return Ok(Some("object_grant_does_not_cover_key"));
    }
    let Some(supplied) = presented_bearer(request) else {
        return Ok(Some("no_bearer_presented"));
    };
    let expected = dashboard.object_token(namespace, policy.item()).await?;
    if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
        Ok(None)
    } else {
        Ok(Some("bearer_does_not_match_namespace_item"))
    }
}

/// A stable refusal code distinguishing missing publisher policy, absent
/// bearer, and a bearer that does not match the declared publisher item.
#[derive(Debug, Clone, Copy)]
enum ReleaseRefusal {
    /// `release_api.publishers` declares nothing that covers this key.
    NoPublisher,
    /// The request carried no `Authorization: Bearer` at all.
    NoBearer,
    /// A bearer was presented and is not the publisher item's token.
    BearerMismatch,
}

impl ReleaseRefusal {
    /// The code the 401 body carries. Stable, because it is what a script and
    /// a runbook match on.
    fn code(self) -> &'static str {
        match self {
            Self::NoPublisher => "no_publisher_for_key",
            Self::NoBearer => "no_bearer_presented",
            Self::BearerMismatch => "bearer_does_not_match_publisher_item",
        }
    }
}

/// The decision one object request reached: `None` authorized, `Some(code)`
/// refused with a reason, `Err` the authority could not be consulted, with
/// the cause the 503 carries.
pub(crate) type ObjectDecision = Result<Option<&'static str>, AuthorityUnavailable>;

/// Authenticate one immutable release publisher after resolving the exact
/// product prefix, and say why when it refuses.
///
/// The former global object token is never consulted.
pub(crate) async fn authorize_release(
    dashboard: &Dashboard,
    request: &Request,
    key_or_prefix: &str,
    list: bool,
) -> ObjectDecision {
    config::release_api_publishers().map_err(|refusals| {
        AuthorityUnavailable::new(format!(
            "release_api.publishers is refused: {}",
            refusals.join("; ")
        ))
    })?;
    let lookup = if list {
        config::release_publisher_for_list(key_or_prefix)
            .map(|found| found.map(|(policy, _)| policy))
    } else {
        config::release_publisher_for_key(key_or_prefix)
    };
    // A declaration that cannot be read is the authority being unavailable,
    // which answers 503, never "no publisher", which answers 401 and sends
    // the operator to declare a publisher that may already be declared.
    let policy = lookup.map_err(|refusal| {
        AuthorityUnavailable::new(format!("release_api.publishers is refused: {refusal}"))
    })?;
    let Some(policy) = policy else {
        // The key, not the bearer: nothing was even looked up to compare
        // against, and the repair is a `release_api.publishers` entry.
        tracing::warn!(
            key = key_or_prefix,
            list,
            reason = ReleaseRefusal::NoPublisher.code(),
            "release request refused: no declared publisher covers this key"
        );
        return Ok(Some(ReleaseRefusal::NoPublisher.code()));
    };
    let Some(supplied) = presented_bearer(request) else {
        tracing::warn!(
            key = key_or_prefix,
            list,
            item = policy.item(),
            reason = ReleaseRefusal::NoBearer.code(),
            "release request refused: no bearer presented for this publisher item"
        );
        return Ok(Some(ReleaseRefusal::NoBearer.code()));
    };
    let expected = dashboard.release_token(policy.item()).await?;
    if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
        return Ok(None);
    }
    // Log lengths only, never credential bytes or prefixes. This distinguishes
    // differing lengths without exposing either bearer.
    tracing::warn!(
        key = key_or_prefix,
        list,
        item = policy.item(),
        expected_len = expected.len(),
        supplied_len = supplied.len(),
        reason = ReleaseRefusal::BearerMismatch.code(),
        "release request refused: the presented bearer is not this publisher item's token"
    );
    Ok(Some(ReleaseRefusal::BearerMismatch.code()))
}
