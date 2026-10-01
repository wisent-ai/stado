//! Which release families one version directory completes on disk.

use std::collections::BTreeSet;
use std::path::Path;

use crate::providers::local::disk_cleanup::release_store::{
    family_key, platform_archive_name, platform_manifest_name, ReleaseFamily, PLATFORM_SUMS_NAME,
    SIGNED_FAMILY,
};

pub(in crate::providers::local::disk_cleanup::release_store) fn complete_families(
    version_path: &Path,
    product: &str,
    version: &str,
) -> BTreeSet<&'static str> {
    let mut families = BTreeSet::new();
    let Ok(platforms) = std::fs::read_dir(version_path) else {
        return families;
    };
    for platform_entry in platforms.flatten() {
        let platform_path = platform_entry.path();
        if !platform_path.is_dir() {
            continue;
        }
        let platform = platform_entry.file_name().to_string_lossy().to_string();
        let installer = [
            platform_archive_name(product, version, &platform),
            platform_manifest_name(&platform),
            PLATFORM_SUMS_NAME.to_string(),
        ];
        if installer
            .iter()
            .all(|name| platform_path.join(name).is_file())
        {
            families.insert(family_key(ReleaseFamily::Installer));
        }
        if SIGNED_FAMILY
            .iter()
            .all(|name| platform_path.join(name).is_file())
        {
            families.insert(family_key(ReleaseFamily::Signed));
        }
    }
    families
}
