mod conflict;
mod objects;
mod pipeline;

use super::CatalogIdentity;
use crate::deploy::products::Product;
use crate::deploy::DeployError;
use pipeline::pipeline_catalog_identity;

pub(crate) use conflict::coordinate_revision_conflict;
pub(crate) use objects::missing_release_objects;

/// The delivered identity of one product coordinate, read from the signed
/// release `stado release submit` publishes there.
pub(crate) async fn catalog_identity(
    product: &Product,
    version: &str,
    platform: &str,
) -> Result<CatalogIdentity, DeployError> {
    pipeline_catalog_identity(product, version, platform).await
}
