//! Versioned, immutable artifact registry.
//!
//! Port of the `stado/artifacts/` package: [`registry`] (ArtifactRegistry
//! over blob storage), [`validation`] (manifest checks), and [`adapters`]
//! (type-specific verification). The domain models (ArtifactRef,
//! ArtifactManifest, canonical JSON) live in [`crate::artifacts_models`]
//! and are re-exported here so `stado::artifacts::ArtifactManifest` works
//! like Python's `stado.artifacts.ArtifactManifest`.

pub mod adapters;
pub mod registry;
pub mod validation;

pub use crate::artifacts_models::{
    ArtifactError, ArtifactLocation, ArtifactManifest, ArtifactProducer, ArtifactRef,
    ArtifactVerification, VerificationReport,
};
pub use adapters::{get_adapter, ArtifactAdapter};
pub use registry::{ArtifactRegistry, RegistryError};
pub use validation::validate_manifest;
