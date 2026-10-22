//! The bearer this client presents for a product namespace: `STADO_API_TOKEN`,
//! else the owner-only file `STADO_API_TOKEN_FILE` names, else the file
//! `storage.stado.token_file` names. It is also the secret access key that
//! namespace signs S3 requests with, which is how `stado storage url
//! --presign` and `stado storage s3-endpoint` sign without a second
//! credential.

use crate::cli::storage::*;

fn config_refusal(message: impl Into<String>) -> CmdError {
    CmdError::click(message.into()).stating(crate::primitives::failure::FailureCode::Config)
}

/// The token file this client reads when `STADO_API_TOKEN` is not set: the
/// one `STADO_API_TOKEN_FILE` names, else the one the configuration names.
fn token_file() -> Result<String, CmdError> {
    let named = match std::env::var("STADO_API_TOKEN_FILE") {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            crate::config::wc_stado_storage_token_file().to_string()
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(config_refusal("STADO_API_TOKEN_FILE must be valid Unicode"))
        }
    };
    if named.trim().is_empty() {
        return Err(config_refusal(
            "STADO_API_TOKEN, STADO_API_TOKEN_FILE or storage.stado.token_file is required to \
             reach the object API",
        ));
    }
    Ok(named)
}

/// Read one owner-only, single-line token file.
fn read_token_file(named: &str) -> Result<String, CmdError> {
    let path = crate::config_file::expand_tilde(named.trim());
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        CmdError::click(format!(
            "cannot inspect STADO_API_TOKEN_FILE {}: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(config_refusal(format!(
            "STADO_API_TOKEN_FILE must be a regular file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(config_refusal(format!(
                "STADO_API_TOKEN_FILE must be owner-only (chmod 600): {}",
                path.display()
            )));
        }
    }
    let value = std::fs::read_to_string(&path).map_err(|error| {
        CmdError::click(format!(
            "cannot read STADO_API_TOKEN_FILE {}: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let token = value.trim();
    if token.is_empty()
        || token
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
    {
        return Err(config_refusal("STADO_API_TOKEN_FILE is empty or malformed"));
    }
    Ok(token.to_string())
}

/// The bearer this client presents for product namespaces.
pub(in crate::cli::storage) fn namespace_bearer() -> Result<String, CmdError> {
    match std::env::var("STADO_API_TOKEN") {
        Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Ok(_) | Err(std::env::VarError::NotPresent) => read_token_file(&token_file()?),
        Err(std::env::VarError::NotUnicode(_)) => Err(config_refusal(
            "STADO_API_TOKEN must be valid Unicode when STADO_API_URL is configured",
        )),
    }
}
