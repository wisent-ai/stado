//! Argument validation: the account name, free text, the login shell, the
//! initial password and the registry's SSH destination.

use std::sync::LazyLock;

use crate::deploy::{py_str_repr, DeployError};

static USERNAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9_-]{0,30}$").expect("static regex compiles"));
static SSH_TARGET_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9_.:@\[\]-]+$").expect("static regex compiles"));

/// Validate a portable, non-system macOS/Linux account name
/// (Python `validate_username`).
pub fn validate_username(username: &str) -> Result<(), DeployError> {
    if !USERNAME_RE.is_match(username) {
        return Err(DeployError(
            "username must start with a lowercase letter, contain only \
             lowercase letters, digits, '_' or '-', and be at most 31 characters"
                .to_string(),
        ));
    }
    Ok(())
}

/// Python `_validate_text`.
pub(super) fn validate_text(
    value: &str,
    label: &str,
    max_length: usize,
) -> Result<(), DeployError> {
    if value.is_empty()
        || value.chars().count() > max_length
        || value.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n'))
    {
        return Err(DeployError(format!(
            "{label} must be 1-{max_length} characters without control newlines"
        )));
    }
    Ok(())
}

/// Python `_validate_shell`: empty is allowed (host OS default).
pub fn validate_shell(shell: &str) -> Result<(), DeployError> {
    if shell.is_empty() {
        return Ok(());
    }
    validate_text(shell, "shell", 255)?;
    if !shell.starts_with('/') {
        return Err(DeployError("shell must be an absolute path".to_string()));
    }
    Ok(())
}

/// Python `_validate_password`.
pub fn validate_password(password: &str) -> Result<(), DeployError> {
    let length = password.chars().count();
    if !(8..=1024).contains(&length) {
        return Err(DeployError(
            "initial password must be between 8 and 1024 characters".to_string(),
        ));
    }
    if password.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')) {
        return Err(DeployError(
            "initial password must not contain NUL or newlines".to_string(),
        ));
    }
    Ok(())
}

/// Python `_validate_ssh_target`.
pub fn validate_ssh_target(ssh_target: &str) -> Result<(), DeployError> {
    if ssh_target.is_empty() || ssh_target.starts_with('-') || !SSH_TARGET_RE.is_match(ssh_target) {
        return Err(DeployError(format!(
            "unsafe SSH destination in registry: {}",
            py_str_repr(ssh_target)
        )));
    }
    Ok(())
}
