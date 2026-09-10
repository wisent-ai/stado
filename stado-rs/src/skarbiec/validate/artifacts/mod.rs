//! The two verifiers whose grants reach stored artifacts: the object store a
//! host reads and writes, and the release channel a build is published to.

mod object;
mod release;

pub use object::validate_object_verifier;
pub use release::validate_release_verifier;
