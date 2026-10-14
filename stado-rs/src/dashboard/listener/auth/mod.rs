//! Who one request is allowed to be: the bearer caches ([`tokens`]), the
//! object and release publisher grants ([`object`]), the service and machine
//! client grants ([`client`]), the vault consultations every bearer read is
//! one of ([`vault`]), and the comparison every one of them uses.

mod client;
mod object;
mod tokens;
mod vault;

use sha2::{Digest, Sha256};

pub(crate) use client::{authenticate_machine_client, authorize_service, machine_result_target};
pub(crate) use object::{
    authorize_host_health, authorize_object, authorize_release, release_object_namespace,
    release_upload_target_key,
};
pub(crate) use tokens::CachedObjectToken;
pub(crate) use vault::{AuthorityUnavailable, VaultConsultations};

/// The bearer a request presents, or `None` when it presents none. Every
/// authorizer asks this before it reads the item to compare against: a
/// request with no bearer is refused without a vault read, so neither an
/// unauthenticated caller nor a vault that stands holds it.
pub(crate) fn presented_bearer(
    request: &crate::dashboard::listener::http::Request,
) -> Option<&str> {
    request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let left = Sha256::digest(left);
    let right = Sha256::digest(right);
    let mut difference = 0;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}
