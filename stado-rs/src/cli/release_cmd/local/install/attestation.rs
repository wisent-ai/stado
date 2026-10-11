//! The receipt the fleet's provenance check reads, left before an install
//! replaces the name.
//!
//! `cli::service_converge::attest_installed` decides provenance by byte
//! comparing the installed file against
//! `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, which
//! `deploy::host_release` writes when IT delivers. `release local install` is
//! the other delivery endpoint and it staged nothing, so a binary it installed
//! read `unattested` forever after — even though the archive was verified
//! against the contract digest.
//!
//! A host whose deliveries plainly ran — its `~/.stado/bin` carries this
//! command's dated backups and its `stado.release-version` handshake — would
//! otherwise hold nothing recent under `~/.stado/releases/stado`. `stado
//! release version show` then reports the host's binary as bytes the fleet
//! cannot attest, and the remediation it prints — deliver a published version
//! — is the thing that has just happened.

use std::path::Path;

/// Stage `source`'s bytes as the attestation copy of `name` at the delivered
/// version: `stado_version` for Stado, `WISENT_VERSION` for another product.
/// Never fatal: the archive is verified and the install is the point, so a
/// receipt that cannot be written is named on stdout, prefixed with `verb`,
/// and the delivery continues.
pub(super) fn stage(verb: &str, name: &str, stado_version: Option<&str>, source: &Path) {
    let delivered = match stado_version {
        Some(version) => Some(version.to_string()),
        None => std::env::var("WISENT_VERSION")
            .ok()
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty()),
    };
    match (delivered, crate::self_update::platform_triple_short()) {
        (Some(version), Ok(platform)) => {
            if let Err(error) =
                crate::self_update::stage_for_attestation(name, &version, platform, source)
            {
                println!(
                    "{verb}: {name} {version} root bytes are verified but its \
                     attestation copy could not be staged, so `stado release version show` will \
                     read it as unattested: {error}"
                );
            }
        }
        (None, _) => println!(
            "{verb}: WISENT_VERSION is unset, so no attestation copy was staged \
             and `stado release version show` will read {name} as unattested"
        ),
        (_, Err(error)) => println!(
            "{verb}: this platform has no release triple ({error}), so no \
             attestation copy was staged for {name}"
        ),
    }
}
