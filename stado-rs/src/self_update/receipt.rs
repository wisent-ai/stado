//! The receipt: the provenance copy the fleet's attestation check reads, so
//! a self-installed binary is not reported as untrustworthy bytes.

use std::path::Path;

use crate::self_update::SelfUpdateError;

/// Copy one verified release member to the coordinate the fleet's provenance
/// check reads, creating `<binary>/<version>/<platform>/` beneath
/// `$HOME/.stado/releases`.
///
/// The bytes are the extracted archive member — the same file
/// [`replace_verified`] installs — so the staged copy is byte-identical to
/// the installed one and `cmp -s` in `attest_installed` matches. Put there
/// whole and durably ([`crate::primitives::document::persist_copy`]), so a
/// reader never sees a partial copy at the coordinate it attests against,
/// and a host that loses power keeps the copy it attested.
///
/// [`replace_verified`]: super::swap::replace::replace_verified
pub(crate) fn stage_for_attestation(
    name: &str,
    version: &str,
    platform: &str,
    verified: &Path,
) -> Result<(), SelfUpdateError> {
    let home = crate::config_file::home().map_err(|cause| {
        SelfUpdateError::Fetch(format!("the attestation copy has nowhere to go: {cause}"))
    })?;
    let destination = home
        .join(".stado")
        .join("releases")
        .join(name)
        .join(version)
        .join(platform)
        .join(name);
    crate::primitives::document::persist_copy(
        &destination,
        verified,
        crate::primitives::file_mode::program(),
    )?;
    Ok(())
}
