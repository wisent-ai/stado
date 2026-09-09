//! Dedicated verifier-grant constructors. Each verifier is an auth boundary:
//! it enforces its exact consumer name and a token file distinct from every
//! other grant, and it never routes through the credential store selector.
//!
//! Every grant here is provisioned on disk by the fleet and stays there, so
//! each one declares `GrantMode::RereadPerRequest`: a rotated verifier grant is
//! picked up without restarting, and none of these files is ever erased.
//!
//! `api` holds the per-boundary API verifiers, `key_readers` the two
//! single-credential readers, and `provider` the per-domain integration grant.
//! Every constructor is an inherent method on the client type, so callers
//! keep naming it through the type and this split needs no re-export.

mod api;
mod key_readers;
mod provider;
