//! Who one request is allowed to be: the bearer caches ([`tokens`]), the
//! object and release publisher grants ([`object`]), the service and machine
//! client grants ([`client`]), and the comparison every one of them uses.

mod client;
mod object;
mod tokens;

use sha2::{Digest, Sha256};

pub(crate) use client::{authenticate_machine_client, authorize_service, machine_result_target};
pub(crate) use object::{
    authorize_host_health, authorize_object, authorize_release, release_object_namespace,
    release_upload_target_key,
};
pub(crate) use tokens::CachedObjectToken;

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let left = Sha256::digest(left);
    let right = Sha256::digest(right);
    let mut difference = u8::default();
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == u8::default()
}
