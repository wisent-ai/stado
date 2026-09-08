//! Immutable artifact registry domain models.
//!
//! Port of `stado/artifacts/models.py` ONLY — the frozen dataclasses and the
//! canonical-JSON serialization. The registry/validation/adapter logic
//! ported from `registry.py`, `validation.py` and `adapters/` lives in
//! [`crate::artifacts`].
//!
//! Canonical JSON is byte-compatible with Python
//! `json.dumps(obj, sort_keys=True, separators=(",", ":"))` (including
//! `ensure_ascii=True` escaping of every char >= 0x7f as \uXXXX), and
//! [`ArtifactManifest::manifest_sha256`] is the SHA-256 of that byte string.

mod error;
mod manifest;
mod records;
mod reference;

pub use error::ArtifactError;
pub use manifest::ArtifactManifest;
pub use records::{ArtifactLocation, ArtifactProducer, ArtifactVerification, VerificationReport};
pub use reference::ArtifactRef;
