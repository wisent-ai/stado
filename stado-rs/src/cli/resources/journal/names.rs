//! The names an operation and its artifacts are allowed to carry, and the one
//! remote path built from them. No component composes a storage path itself,
//! so no operation id or artifact name reaches storage unvalidated.

use crate::cli::CmdError;

pub(super) fn validate_operation_id(value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CmdError::usage(format!("invalid operation id {value:?}")));
    }
    Ok(())
}

pub(super) fn validate_artifact_name(value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains("..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        return Err(CmdError::click(format!(
            "invalid operation artifact {value:?}"
        )));
    }
    Ok(())
}

pub(super) fn remote_path(operation_id: &str, name: &str) -> String {
    format!("operations/{operation_id}/{name}")
}
