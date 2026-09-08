//! The managed-version declaration an `auto_declare` recipe earns once its
//! build succeeded and recorded an exact version.

use crate::deploy::products;
use crate::targets::Registry;

/// Declare `version` as the managed version of the recipe's product on every
/// registry host of `platform`, and say so once per host.
///
/// This calls [`crate::cli::host::declare_version`] itself — the same
/// function `stado host declare-version` runs, fence and validation included,
/// which also means it prints the CLI's own confirmation line per host
/// alongside the daemon log line. A recipe declares the product its NAME
/// selects: a recipe named after nothing the fleet declares has artifacts to
/// keep and no product to move, and says so instead of guessing.
///
/// The return value is what
/// [`BuildRun::declared`](crate::targets::BuildRun::declared) records, so it
/// is true only when every matching host took the declaration: a partial
/// fleet is not a declared version.
pub(super) async fn declare_on_platform(
    registry: &Registry,
    recipe: &str,
    platform: &str,
    version: &str,
    log: &dyn Fn(&str),
) -> bool {
    let product = match products::product(recipe) {
        Ok(product) => product,
        Err(exc) => {
            log(&format!(
                "build {recipe}: auto-declare skipped for {platform}: {exc}"
            ));
            return false;
        }
    };
    if !product.platforms.iter().any(|word| word == platform) {
        log(&format!(
            "build {recipe}: auto-declare skipped for {platform}: {} is not published for it",
            product.name
        ));
        return false;
    }
    let hosts: Vec<String> = registry
        .targets
        .iter()
        .filter(|target| target.release_platform == platform)
        .map(|target| target.name.clone())
        .collect();
    if hosts.is_empty() {
        log(&format!(
            "build {recipe}: auto-declare skipped for {platform}: no registry host reports it"
        ));
        return false;
    }
    let mut declared_everywhere = true;
    for host in &hosts {
        match crate::cli::host::declare_version(host, &product.name, Some(version), false, false)
            .await
        {
            Ok(()) => log(&format!(
                "build {recipe}: declared {} {version} on {host} ({platform})",
                product.name
            )),
            Err(exc) => {
                declared_everywhere = false;
                log(&format!(
                    "build {recipe}: declaring {} {version} on {host} ({platform}) failed: {exc}",
                    product.name
                ));
            }
        }
    }
    declared_everywhere
}
