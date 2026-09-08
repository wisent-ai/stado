//! The documents a release publication is made of: the source-revision claims
//! a coordinate and a version attest, the signed manifest, and the
//! registry-owned rollout policy the fleet converges on.

pub(super) mod manifest;
pub(super) mod policy;
pub(super) mod revision;
