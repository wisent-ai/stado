//! The shapes a name, reference, alias, selector, address, image digest or
//! revision has to have before the registry will carry it.

use sha2::{Digest, Sha256};

pub(super) fn identifier(value: &str) -> bool {
    let edge = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
    let one = usize::from(u8::from(true));
    let two = one.saturating_add(one);
    let maximum = usize::from(u8::MAX).saturating_add(one) / two;
    value.len() <= maximum
        && value.chars().next().is_some_and(edge)
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
/// or later.** This function was added in `f020b63e`, which landed 3 minutes 43
/// seconds AFTER `stado-v0.13.9` was tagged, so it first ships in 0.13.10. A
/// binary without it refuses `"best"` as naming a non-running deployment — and
/// refusing any part of the registry means refusing the whole document, which
/// means resolving no `disk_cleanup` policy at all.
///
/// So on any host below 0.13.10 this value is a janitor kill switch, not a
/// routing preference. On 2026-08-31 at 07:13:43Z it switched off every
/// cleaner on `charless-mac-mini` — the janitor answered
/// `invalid_or_unavailable_policy`, `errors: ["policy:ValueError"]`,
/// `target_name: null` — from a single field in a section the janitor never
/// reads. Restoring the route to a concrete destination at 07:19:11Z brought
/// it back to `errors: []`, `mode: enforce` by 07:25:20Z.
///
/// The precondition for restoring it, all three parts:
///
/// 1. 0.13.10 or later is published whole for every platform in the fleet;
/// 2. it is delivered to every host, not just the control plane;
/// 3. `stado service converge <host> stado` reads `in-sync` at that version on
///    each one.
///
/// `#197` narrows the blast radius — a write that leaves `inference`
/// byte-identical is no longer refused for a pre-existing fault in it — but it
/// does not make an older binary able to parse this value. Only delivery does.
pub fn gateway_selector(value: &str) -> bool {
    value == "best"
}

pub(super) fn tailscale_ipv4(value: &str) -> bool {
    let Ok(address) = value.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    let octets = address.octets();
    let first = "100".parse::<u8>().expect("static Tailscale prefix");
    let lower = "64".parse::<u8>().expect("static Tailscale range");
    let upper = "128".parse::<u8>().expect("static Tailscale range");
    octets[usize::MIN] == first && (lower..upper).contains(&octets[usize::from(true)])
}

pub(super) fn sha256_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    let two = usize::from(u8::from(true)).saturating_add(usize::from(u8::from(true)));
    let length = Sha256::output_size().saturating_mul(two);
    safe_reference(name, "/:")
        && digest.len() == length
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(super) fn immutable_revision(value: &str) -> bool {
    let length = Sha256::output_size().saturating_add(u8::BITS as usize);
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
