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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn installability_needs_all_three_files_from_one_platform() {
        let home = tempfile::tempdir().unwrap();
        let complete = home.path().join("0.15.21");
        write(
            &complete.join("darwin-arm64/stado-v0.15.21-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(
            &complete.join("darwin-arm64/release-manifest-darwin-arm64.json"),
            b"{}",
        );
        write(&complete.join("darwin-arm64/SHA256SUMS"), b"sums");
        assert_eq!(
            complete_families(&complete, "stado", "0.15.21"),
            BTreeSet::from([family_key(ReleaseFamily::Installer)])
        );

        // The interrupted publish: the create-only claim and nothing else.
        let claim = home.path().join("0.16.1");
        write(&claim.join("darwin-arm64/source-revision.json"), b"{}");
        assert!(complete_families(&claim, "stado", "0.16.1").is_empty());

        // Archive and sums but no platform manifest: `install-stado.sh` reads
        // the manifest first and verifies the archive against its digest, so
        // this is a download that fails after the bytes, not a release.
        let partial = home.path().join("0.16.0");
        write(
            &partial.join("darwin-arm64/stado-v0.16.0-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(&partial.join("darwin-arm64/SHA256SUMS"), b"sums");
        assert!(complete_families(&partial, "stado", "0.16.0").is_empty());

        // The three files exist, but split across two platforms: neither
        // platform is installable, and a caller asks for one platform.
        let split = home.path().join("0.16.2");
        write(
            &split.join("darwin-arm64/stado-v0.16.2-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(
            &split.join("linux-amd64/release-manifest-linux-amd64.json"),
            b"{}",
        );
        write(&split.join("linux-amd64/SHA256SUMS"), b"sums");
        assert!(complete_families(&split, "stado", "0.16.2").is_empty());

        // A version whose archive is named for another version is not this
        // version's release, which is what an unfinalised multipart upload
        // directory looks like from here.
        let mismatched = home.path().join("0.16.3");
        write(
            &mismatched.join("darwin-arm64/stado-v0.16.2-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(
            &mismatched.join("darwin-arm64/release-manifest-darwin-arm64.json"),
            b"{}",
        );
        write(&mismatched.join("darwin-arm64/SHA256SUMS"), b"sums");
        assert!(complete_families(&mismatched, "stado", "0.16.3").is_empty());
    }

    /// The web-product shape, which had no pin at all: `preferences-landing`
    /// publishes `release.json` + `release.sig` + `release.tar.gz` under a
    /// `web` platform and no installer family ever, so before this it could
    /// only be protected by `keep_newest` - a count of directories that says
    /// nothing about whether any of them can be deployed.
    #[test]
    fn a_signed_web_release_is_recognised_as_complete() {
        let home = tempfile::tempdir().unwrap();
        let release = home.path().join("0.1.1");
        write(&release.join("web/release.json"), b"{}");
        write(&release.join("web/release.sig"), b"sig");
        write(&release.join("web/release.tar.gz"), b"archive");
        write(&release.join("web/source-revision.json"), b"{}");
        assert_eq!(
            complete_families(&release, "preferences-landing", "0.1.1"),
            BTreeSet::from([family_key(ReleaseFamily::Signed)])
        );

        // The signature is what makes it deployable; without it the manifest
        // and archive are unverifiable bytes.
        let unsigned = home.path().join("0.1.2");
        write(&unsigned.join("web/release.json"), b"{}");
        write(&unsigned.join("web/release.tar.gz"), b"archive");
        assert!(complete_families(&unsigned, "preferences-landing", "0.1.2").is_empty());
    }
}
