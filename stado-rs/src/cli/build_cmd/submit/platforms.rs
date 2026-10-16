//! Narrow one build to the requested platform and delivery dependency graph.

use super::SourceReading;
use crate::cli::CmdError;
use crate::release_pipeline::ProductManifest;

/// Keep only selected platforms and their deliveries. An unknown platform or
/// a retained delivery depending on a dropped one refuses before any write.
pub(super) fn restrict_platforms(
    mut reading: SourceReading,
    platforms: &[String],
) -> Result<SourceReading, CmdError> {
    let declared: Vec<String> = reading.manifest.platforms.keys().cloned().collect();
    if let Some(unknown) = platforms
        .iter()
        .find(|platform| !declared.contains(platform))
    {
        return Err(CmdError::usage(format!(
            "{} declares no platform {unknown}; declared: {}",
            reading.manifest.product,
            declared.join(", ")
        )));
    }
    let manifest = &mut reading.manifest;
    manifest
        .platforms
        .retain(|platform, _| platforms.contains(platform));
    let dropped: Vec<String> = manifest
        .deliveries
        .iter()
        .filter(|delivery| !platforms.contains(&delivery.platform))
        .map(|delivery| delivery.name.clone())
        .collect();
    manifest
        .deliveries
        .retain(|delivery| platforms.contains(&delivery.platform));
    if let Some((delivery, waits_on)) = manifest.deliveries.iter().find_map(|delivery| {
        delivery
            .after
            .iter()
            .find(|name| dropped.contains(name))
            .map(|name| (delivery.name.clone(), name.clone()))
    }) {
        return Err(CmdError::usage(format!(
            "delivery {delivery} waits on {waits_on}, which runs on a platform this build leaves out; build that platform too"
        )));
    }
    reading.manifest_bytes = serde_json::to_vec_pretty(&reading.manifest)?;
    reading.product = ProductManifest::Release(reading.manifest.clone());
    Ok(reading)
}
