//! Declared versions, staged releases, and their provenance.

pub(in crate::cli::host) mod activate;
pub(in crate::cli::host) mod platform;
pub(in crate::cli::host) mod provenance;
pub(in crate::cli::host) mod versions;

use crate::cli::CmdError;

pub(in crate::cli::host) fn release_component(kind: &str, value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CmdError::usage(format!(
            "{kind} must contain only letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(())
}
