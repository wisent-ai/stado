//! `service file-sync` and `service file-fetch`: one file into or out of a
//! managed service's target home, byte-exact both ways.

use super::*;

pub(crate) mod fetch;
pub(crate) mod sync;

/// Replace one local path with these bytes, owner-only, through a rename.
fn write_owner_only(destination: &str, content: &[u8]) -> Result<(), CmdError> {
    let path = std::path::Path::new(destination);
    let staged = path.with_extension("stado-file-fetch");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    std::fs::write(&staged, content)
        .map_err(|error| CmdError::click(format!("cannot stage {destination}: {error}")))?;
    #[cfg(unix)]
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| CmdError::click(format!("cannot protect {destination}: {error}")))?;
    std::fs::rename(&staged, path)
        .map_err(|error| CmdError::click(format!("cannot install {destination}: {error}")))
}
