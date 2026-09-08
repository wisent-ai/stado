use crate::deploy::products::Product;
use crate::deploy::DeployError;

/// Which objects one published coordinate declares but does not actually
/// have, in the order the declaration names them.
///
/// `SHA256SUMS` is the declaration. The publisher writes one line per file it
/// placed in the release directory, so that file names the exact set a
/// complete version holds — and it is the only thing at the coordinate that
/// does. The per-platform manifest carries product, version, platform,
/// source commit and the archive digest; nothing in it lists the binaries.
///
/// Reading the declaration back and probing every name it lists is what sees
/// a half-published version. The archive, the manifest and `SHA256SUMS` are
/// written first, so a version that lost five of its six binaries still
/// answers for all three and looks, to every check that came before this one,
/// exactly like a finished release. `stado/0.11.0/darwin-arm64` is in that
/// state permanently: 4 objects of 9, missing `wc`, `stado-coverage`,
/// `stado-fix`, `stado-watchdog` and `stado-mcp`. The objects are immutable,
/// so an interrupted publish can never be completed, and delivering from it
/// installs a version whose binaries do not exist.
///
/// `stado/0.10.0/darwin-arm64` is a different shape and is refused earlier:
/// it holds nothing at all — 0 of 9 — so the manifest check above rejects it
/// before this function runs, while its `linux-amd64` leg is a complete 9.
/// Both were measured object by object through the release API on
/// 2026-08-30; only 0.11.0 needed this check to be caught.
///
/// For the `stado` product the declaration is checked against
/// [`crate::self_update::RELEASE_BINARIES`] as well, because a `SHA256SUMS`
/// that is itself short would otherwise shrink the verified set to whatever
/// happened to be listed.
pub(crate) async fn missing_release_objects(
    product: &Product,
    version: &str,
    platform: &str,
) -> Result<Vec<String>, DeployError> {
    let base = format!(
        "stado://releases/{}/{version}/{platform}",
        product.source.product
    );
    // Which contract this coordinate is held to is decided by which publisher
    // wrote it, and that is readable from the coordinate itself.
    //
    // Two publishers write here. The tag's release train writes the nine
    // platform objects -- six binaries, `SHA256SUMS`, the platform manifest
    // and `stado-v<version>-<platform>.tar.gz`. `stado release worker` writes
    // an archive-based signed release: `release.json`, `release.sig`,
    // `release.tar.gz` and `qualification.json`, and no binaries beside them
    // on purpose, because the archive IS the payload.
    //
    // [`catalog_identity`] already knows this and prefers the signed leg for
    // exactly the stated reason: validating the legacy surface "incorrectly
    // demands the legacy sidecar binaries from an archive-based pipeline
    // release". This function did not know it, so it judged every coordinate
    // by the nine-object contract -- and on its first working run in `doctor`
    // it called `0.13.48/darwin-arm64` and `0.13.48/linux-amd64` PARTIAL for
    // want of `SHA256SUMS`, when both hold a complete signed release and no
    // tag train has ever run for that version. An audit whose first act is to
    // condemn a healthy coordinate teaches people to close it.
    let signed_uri = format!("{base}/{}", crate::release_control::RELEASE_MANIFEST_NAME);
    if crate::cli::storage::release_object_present(&signed_uri)
        .await
        .map_err(|error| DeployError(error.to_string()))?
    {
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
        return Ok(missing);
    }
    let sums_name = crate::self_update::SHA256SUMS_NAME;
    let sums_uri = format!("{base}/{sums_name}");
    let declares_binary_set = product.source.product == "stado";
    let sums_present = crate::cli::storage::release_object_present(&sums_uri)
        .await
        .map_err(|error| DeployError(error.to_string()))?;
    if !sums_present {
        // No signed leg and no checksum declaration: a train-shaped coordinate
        // that lost the file naming its own contents. Every published stado
        // version from that publisher carries one, so its absence is a missing
        // object rather than a product that never had the file.
        return Ok(if declares_binary_set {
            vec![sums_name.to_string()]
        } else {
            Vec::new()
        });
    }
    let sums = crate::cli::storage::fetch_object(&sums_uri)
        .await
        .map_err(|error| {
            DeployError(format!(
                "cannot read the release declaration at {sums_uri}: {error}"
            ))
        })?;
    let sums = String::from_utf8(sums)
        .map_err(|_| DeployError(format!("{sums_uri} is not UTF-8 text")))?;
    let declared = crate::self_update::parse_sha256sums(&sums)
        .map_err(|error| DeployError(format!("{sums_uri}: {error}")))?;
    let mut names: Vec<String> = declared.into_keys().collect();
    if declares_binary_set {
        for &name in crate::self_update::RELEASE_BINARIES {
            if !names.iter().any(|declared| declared == name) {
                names.push(name.to_string());
            }
        }
        names.sort();
    }
    // One round trip for the coordinate, not one per name. The names are known
    // before any of them is probed and the probes do not depend on each other,
    // so serialising them only multiplied the wall clock by nine: the standing
    // channel audit in `doctor` has twelve coordinates to read and was timing
    // out before it finished the first.
    //
    // Order is preserved by collecting the answers positionally rather than by
    // whichever reply lands first, because the absent list is read by a person
    // and `SHA256SUMS` order is the order they will look for.
    let probes = names.iter().map(|name| {
        let uri = format!("{base}/{name}");
        async move { crate::cli::storage::release_object_present(&uri).await }
    });
    let present = futures::future::join_all(probes).await;
    let mut missing = Vec::new();
    for (name, answer) in names.into_iter().zip(present) {
        if !answer.map_err(|error| DeployError(error.to_string()))? {
            missing.push(name);
        }
    }
    Ok(missing)
}
