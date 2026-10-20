//! Reading one unit file the host unit installation owns.

use std::path::Path;

use crate::deploy::service::{parse_local_unit_file, LocalUnitFile};
use crate::deploy::DeployError;

/// One unit file this installation owns: `None` when it is not there, an
/// error naming it when it cannot be read, is not text or does not parse.
pub(super) fn read_unit(path: &Path, kind: &str) -> Result<Option<LocalUnitFile>, DeployError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DeployError::io(format!("reading {}", path.display()))(
                error,
            ))
        }
    };
    let content = String::from_utf8(bytes)
        .map_err(|error| DeployError(format!("{} is not text: {error}", path.display())))?;
    parse_local_unit_file(&content, kind)
        .map(Some)
        .map_err(|error| error.within(path.display().to_string()))
}
