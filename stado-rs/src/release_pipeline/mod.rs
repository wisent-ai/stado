//! Checked-in product release contract and durable pipeline records.
//!
//! `.wisent-release.json` is the host-independent boundary between a product
//! and Stado.  Repository locations, fleet hosts, storage providers and secret
//! material deliberately do not appear in this schema.

mod contract;
mod records;
mod validate;

pub const PRODUCT_MANIFEST: &str = ".wisent-release.json";
pub const SCHEMA_VERSION: u32 = 1;
pub const RUNNER_PLATFORMS: [&str; 2] = ["darwin-arm64", "linux-amd64"];

pub use contract::catalog::{CatalogSourceIdentity, ReleaseCatalogEntry};
pub use contract::manifest::{
    NonReleaseManifest, ProductManifest, ReleasePipelineManifest, VersionSource,
};
pub use contract::recipe::{
    BuildCommand, Delivery, PipelineChannel, PlatformRecipe, PromotionPolicy, QualityGate,
    ReleaseInput, RuntimeContract,
};
pub use records::receipt::{ArtifactReceipt, BuildReceipt, ReceiptInput, StepReceipt, StepStatus};
pub use records::run::{
    DeliveryRun, DeliveryRunState, PlatformRun, PlatformRunState, ReleaseRun, ReleaseRunState,
};
pub use records::scratch::{tree_bytes, ScratchReceipt, SCRATCH_LEAF};
pub use records::worker::{WorkerInput, WorkerRequest};
pub use validate::manifest::{
    parse_product_manifest, validate_product_manifest, validate_release_manifest,
};
pub use validate::predicates::safe_relative;
pub use validate::roles::{platform_runtime_role, runtime_role, RuntimeRole};
pub use validate::version::{declared_version, validate_catalog_entry};
