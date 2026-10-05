use crate::deploy::products::Product;
use crate::deploy::DeployError;

/// Which objects of one published coordinate's signed release are absent:
/// `release.json`, `release.sig`, `release.tar.gz` and `qualification.json`,
/// the four `stado release worker` writes. The archive is the payload, so no
/// binary is published beside them.
///
/// The four are probed together, one round trip for the coordinate, and the
/// answers are kept in that order so the absent list reads as the contract
/// does.
pub(crate) async fn missing_release_objects(
    product: &Product,
    version: &str,
    platform: &str,
) -> Result<Vec<String>, DeployError> {
    let base = format!(
        "stado://releases/{}/{version}/{platform}",
        product.source.product
    );
    let required = [
        crate::release_control::RELEASE_MANIFEST_NAME,
        crate::release_control::RELEASE_SIGNATURE_NAME,
        crate::release_control::RELEASE_ARCHIVE_NAME,
        crate::release_control::RELEASE_QUALIFICATION_NAME,
    ];
    let probes = required.iter().map(|name| {
        let uri = format!("{base}/{name}");
        async move { crate::cli::storage::release_object_present(&uri).await }
    });
    let present = futures::future::join_all(probes).await;
    let mut missing = Vec::new();
    for (name, answer) in required.iter().zip(present) {
        if !answer.map_err(|error| DeployError(error.to_string()))? {
            missing.push((*name).to_string());
        }
    }
    Ok(missing)
}
