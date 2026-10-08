//! The shapes a name, reference, alias, selector, address, image digest or
//! revision has to have before the registry will carry it.

use sha2::{Digest, Sha256};

pub(super) fn identifier(value: &str) -> bool {
    let edge = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
    value.chars().next().is_some_and(edge)
        && value.chars().next_back().is_some_and(edge)
        && value
            .chars()
            .all(|ch| edge(ch) || matches!(ch, '.' | '-' | '_'))
}

pub(super) fn safe_reference(value: &str, extra: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains("..")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch) || extra.contains(ch))
}

/// A route alias: one or more lowercase identifiers joined by `/`.
///
/// An alias used to be required to carry a `/`, on the theory that its first
/// segment names a purpose or a consumer. That forced every consumer to invent
/// a suffix — `weles/agent/primary` — whose words then changed meaning under a
/// name that stayed, and the name stopped describing anything. A consumer's own
/// name is a complete alias: `weles` is the alias Weles asks for, and which
/// model answers it is this table's business. The purpose rule below still
/// reads the first segment, so a bare `weles` keeps the namespace `weles` and
/// is still refused a model declared for another purpose.
pub(super) fn route_alias(value: &str) -> bool {
    !value.is_empty() && value.split('/').all(identifier)
}
/// The one managed alias a route may name instead of a concrete destination.
///
/// **Do not set a route to `"best"` until every host in the fleet runs 0.13.10
/// or later.** This function landed minutes AFTER the release before it was
/// tagged, so it first ships in that next release. A
/// binary without it refuses `"best"` as naming a non-running deployment — and
/// refusing any part of the registry means refusing the whole document.
///
/// So on any host below 0.13.10 this value refuses the registry document
/// from a single field in a section most readers never use, and restoring
/// the route to a concrete destination brings the document back within
/// minutes.
///
/// The precondition for restoring it, all three parts:
///
/// 1. 0.13.10 or later is published whole for every platform in the fleet;
/// 2. it is delivered to every host, not just the control plane;
/// 3. `stado release version show --host <host> --binary stado` reads `in-sync` at that version on
///    each one.
///
/// `#197` narrows the blast radius — a write that leaves `inference`
/// byte-identical is no longer refused for a pre-existing fault in it — but it
/// does not make an older binary able to parse this value. Only delivery does.
pub fn gateway_selector(value: &str) -> bool {
    value == "best"
}

/// The endpoint host must be an IPv4 address; which network it sits on is the
/// deployment's `visibility`.
pub(super) fn ipv4(value: &str) -> bool {
    value.parse::<std::net::Ipv4Addr>().is_ok()
}

/// An image pinned by a SHA-256 digest: hex that decodes to exactly one
/// SHA-256 output.
pub(super) fn sha256_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    safe_reference(name, "/:")
        && hex::decode(digest).is_ok_and(|bytes| bytes.len() == Sha256::output_size())
}

/// A revision named by its commit hash rather than a branch or tag.
pub(super) fn immutable_revision(value: &str) -> bool {
    hex::decode(value).is_ok_and(|bytes| !bytes.is_empty())
}
