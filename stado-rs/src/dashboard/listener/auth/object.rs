//! Object-plane bearers: the namespace grant one product object authorizes
//! against, the immutable release publisher one release coordinate authorizes
//! against, the host beacon publisher, and which of the four faults refused.

use crate::config;
use crate::dashboard::listener::http::Request;
use crate::dashboard::listener::Dashboard;

use super::constant_time_eq;

/// Accept a bearer only after the request has resolved to one canonical
/// namespace and key boundary. Out-of-scope requests and bearer mismatches are
/// unauthorized; invalid configuration or an unavailable exact item is
/// reported separately so the route can return a redacted 503.
pub(crate) fn release_object_namespace(namespace: &str) -> bool {
    matches!(namespace, "releases" | "sources")
}

/// Return the immutable target governed by one disposable chunk key.
///
/// Release objects remain undeletable. Only the exact staging shape emitted by
/// the chunked uploader can be removed, and it is authenticated against the
/// final target rather than against an independently chosen prefix.
pub(crate) fn release_upload_target_key(key: &str) -> Option<&str> {
    let (target, suffix) = key.split_once(".__stado_upload/")?;
    let mut parts = suffix.split('/');
    let upload_id = parts.next()?;
    let chunk_index = parts.next()?;
    if target.is_empty()
        || upload_id.len() != 64
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || chunk_index.len() != 8
        || !chunk_index.bytes().all(|byte| byte.is_ascii_digit())
        || parts.next().is_some()
    {
        return None;
    }
    Some(target)
}

/// Route-scoped host beacon publication: the bearer stored as
/// `stado-host-health-api/token` and nothing else. The dashboard resolves it
/// through the same dedicated verifier grant as its object routes, never
/// through the broad coordinator credential. Machine publishers are
/// authorized separately through their exact client policies.
///
/// `Ok(false)` is a rejected bearer. `Err(())` is this service being unable to
/// read the item it compares against — a local, retryable fault that the
/// caller cannot fix by presenting a different credential.
pub(crate) async fn authorize_host_health(
    dashboard: &Dashboard,
    request: &Request,
) -> Result<bool, ()> {
    let expected = dashboard
        .object_token("host-health", crate::config::HOST_HEALTH_API_ITEM)
        .await?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

/// Authorize one object request against the namespace that declares it, and
/// say which of the four faults refused it.
///
/// [`ReleaseRefusal`] already learned this lesson on the release route: one
/// code for every refusal cost a day, because "no declaration", "key outside
/// the declared prefixes", "no bearer at all" and "the wrong bearer" need
/// opposite repairs and read identically. The object route kept collapsing
/// them, and on 2026-09-05 it answered `object_grant_does_not_cover_key` for
/// `stado://spis-crawls/runs/…` on a host whose configuration declares
/// `runs/` with `get` — so the message named the one cause that was not
/// true, and the real one had to be found by excluding hypotheses again.
pub(crate) async fn authorize_object(
    dashboard: &Dashboard,
    request: &Request,
    namespace: &str,
    key_or_prefix: &str,
    list: bool,
    action: &str,
) -> ObjectDecision {
    let namespaces = config::object_api_namespaces().map_err(|_| ())?;
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
    let expected = dashboard.object_token(namespace, policy.item()).await?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let Some(supplied) = authorization.strip_prefix("Bearer ") else {
        return Ok(Some("no_bearer_presented"));
    };
    if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
        Ok(None)
    } else {
        Ok(Some("bearer_does_not_match_namespace_item"))
    }
}

/// Why one release request was refused, as a stable code an operator can act
/// on.
///
/// A bare `{"error":"unauthorized"}` covers three faults that need opposite
/// repairs — no publisher declared for the key, no bearer presented at all,
/// and a bearer that does not match the publisher item — and on 2026-09-03 it
/// cost most of a day. `stado storage stat` answered it for
/// `stado://system/release-catalog/<product>.json` for every product,
/// including ones that publish successfully, while the same publisher bearer
/// authorized `stado://sources/<product>/…` on the same host in the same
/// second. Nothing on either end said which of the three it was, so every
/// hypothesis had to be excluded by experiment: the token values, the
/// publisher declaration on the host, the configuration cache, the token
/// cache, and the host's own build.
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
/// refused with a reason, `Err(())` the authority could not be consulted.
pub(crate) type ObjectDecision = Result<Option<&'static str>, ()>;

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
    config::release_api_publishers().map_err(|_| ())?;
    let policy = if list {
        config::release_publisher_for_list(key_or_prefix).map(|(policy, _)| policy)
    } else {
        config::release_publisher_for_key(key_or_prefix)
    };
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
    let expected = dashboard.release_token(policy.item()).await?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    if supplied.is_empty() {
        tracing::warn!(
            key = key_or_prefix,
            list,
            item = policy.item(),
            reason = ReleaseRefusal::NoBearer.code(),
            "release request refused: no bearer presented for this publisher item"
        );
        return Ok(Some(ReleaseRefusal::NoBearer.code()));
    }
    if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
        return Ok(None);
    }
    // Lengths only, never a prefix of either value: a bearer is credential
    // material and a leading fragment of one is still a fragment of one. The
    // two lengths are enough to separate "a different credential entirely"
    // from "the same credential with a stray byte", which was the live
    // question on the day this line was written.
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
