//! The receipt: the provenance copy the fleet's attestation check reads, so
//! a self-installed binary is not reported as untrustworthy bytes.

use std::path::{Path, PathBuf};

use crate::self_update::SelfUpdateError;

/// Copy one verified release member to the coordinate the fleet's provenance
/// check reads, creating `<binary>/<version>/<platform>/` beneath
/// `$HOME/.stado/releases`.
///
/// The bytes are the extracted archive member — the same file
/// [`replace_verified`] installs — so the staged copy is byte-identical to
/// the installed one and `cmp -s` in `attest_installed` matches. Written to a
/// dot-prefixed name and renamed, so a reader never sees a partial copy at
/// the coordinate it attests against.
///
/// [`replace_verified`]: super::swap::replace::replace_verified
pub(crate) fn stage_for_attestation(
    name: &str,
    version: &str,
    platform: &str,
    verified: &Path,
) -> Result<(), SelfUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::var_os("HOME").ok_or_else(|| {
        SelfUpdateError::Fetch("HOME is unset, so the attestation copy has nowhere to go".into())
    })?;
    let coordinate = PathBuf::from(home)
        .join(".stado")
        .join("releases")
        .join(name)
        .join(version)
        .join(platform);
    std::fs::create_dir_all(&coordinate)?;
    let destination = coordinate.join(name);
    let temporary = coordinate.join(format!(".{name}.staging"));
    std::fs::copy(verified, &temporary)?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
    std::fs::File::open(&temporary)?.sync_all()?;
    if let Err(error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}
