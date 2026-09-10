use crate::targets::ComputeTarget;

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// True for an exact semantic version: three numeric identifiers without
/// leading zeros, plus an optional prerelease.
///
/// Build metadata (`+...`) is rejected rather than tolerated, because a `+`
/// is not a legal release coordinate segment
/// ([`crate::binary::release::canonical_coordinate`]) — a version this accepts and
/// the store cannot address would be a refusal deferred to the host.
pub fn is_exact_semver(version: &str) -> bool {
    let (core, prerelease) = match version.split_once('-') {
        Some((core, rest)) => (core, Some(rest)),
        None => (version, None),
    };
    let mut parts = core.split('.');
    let numeric = |token: &str| {
        !token.is_empty()
            && token.bytes().all(|byte| byte.is_ascii_digit())
            && (token == "0" || !token.starts_with('0'))
    };
    let (Some(major), Some(minor), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if !numeric(major) || !numeric(minor) || !numeric(patch) {
        return false;
    }
    if let Some(prerelease) = prerelease {
        if prerelease.is_empty() {
            return false;
        }
        for identifier in prerelease.split('.') {
            let alphanumeric = identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
            if identifier.is_empty() || !alphanumeric {
                return false;
            }
            if identifier.bytes().all(|byte| byte.is_ascii_digit()) && !numeric(identifier) {
                return false;
            }
        }
    }
    crate::binary::release::canonical_coordinate(version)
}

/// True for a lowercase hex SHA-256.
pub fn is_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The version the registry declares this host must run for this binary.
///
/// One accessor, [`ComputeTarget::declared_version`], shared with the
/// reconciliation `host inventory` reports. Delivery must never carry its
/// own reading of the declaration: two readings that can disagree turn "the
/// host is behind" and "the delivery is refused" into independent answers
/// to the same question.
pub(super) fn declared_version<'a>(target: &'a ComputeTarget, binary: &str) -> Option<&'a str> {
    target
        .declared_version(binary)
        .filter(|version| !version.is_empty())
}

/// The scheme contract for one release origin: HTTPS for every target, or
/// loopback HTTP when the target is its own store. The fetch runs on the
/// target itself, so a loopback origin never crosses a network and can only
/// ever name that host's own store; for every other target the origin leaves
/// the machine, and off-host HTTP is exactly the tamperable path the HTTPS
/// rule exists to close.
pub(super) fn release_origin_allowed(release_api: &str, self_store: bool) -> bool {
    if release_api.starts_with("https://") {
        return true;
    }
    self_store && loopback_http_origin(release_api)
}

/// `http://` naming this machine and nothing else: a loopback IP or
/// `localhost`. The host is parsed, not prefix-matched, so
/// `http://127.0.0.1.evil.example` is not loopback.
pub(crate) fn loopback_http_origin(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}
