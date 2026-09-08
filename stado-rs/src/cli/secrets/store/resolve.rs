//! Where a credential operation resolves to: the admin client, the installed
//! Skarbiec, the owner vault and the keychain launcher.

use serde_json::Value;

use crate::cli::CmdError;

pub(crate) fn client() -> Result<crate::skarbiec::Client, CmdError> {
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|err| CmdError::click(err.to_string()))?;
    crate::skarbiec::Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(|err| CmdError::click(err.to_string()))
}

/// The installed Skarbiec, resolved where every owner-path write resolves it.
pub(crate) fn skarbiec_binary() -> Result<std::path::PathBuf, CmdError> {
    crate::credential_store::owner::binary().map_err(|error| CmdError::click(error.to_string()))
}

/// The one vault an owner write lands in, resolved exactly where
/// [`crate::credential_store::owner`] resolves it.
///
/// This host runs one credential store. A verb that resolved its own path is
/// free to disagree with every other write in the process, and that is how a
/// vault dedicated to a single writer came to sit beside the canonical one
/// holding the only copy of Weles's credentials. Resolution that finds no
/// existing vault file is an error here rather than an invitation to create
/// one: a second vault created quietly is the defect, not the recovery.
pub(crate) fn owner_vault() -> Result<std::path::PathBuf, CmdError> {
    crate::credential_store::owner::vault().map_err(|error| CmdError::click(error.to_string()))
}

const SKARBIEC_LAUNCHER_CANDIDATES: &[&str] = &["$HOME/.stado/bin/skarbiec-keychain-launcher"];

pub(crate) fn skarbiec_launcher() -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    if let Ok(explicit) = std::env::var("SKARBIEC_LAUNCHER") {
        let path = std::path::PathBuf::from(&explicit);
        if !path.is_file() {
            return Err(CmdError::click(format!(
                "SKARBIEC_LAUNCHER names no file: {explicit}"
            )));
        }
        return Ok(path);
    }
    for candidate in SKARBIEC_LAUNCHER_CANDIDATES {
        let path = std::path::PathBuf::from(candidate.replace("$HOME", &home));
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(CmdError::click(format!(
        "no installed Skarbiec launcher at {}",
        SKARBIEC_LAUNCHER_CANDIDATES.join(", ")
    )))
}

pub(crate) fn launcher_json(
    binary: &std::path::Path,
    vault: &std::path::Path,
    arguments: &[&str],
) -> Result<Value, CmdError> {
    let output = std::process::Command::new(binary)
        .args(arguments)
        .env("SKARBIEC_VAULT_FILE", vault)
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output()?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} {} failed: {}",
            binary.display(),
            arguments.first().copied().unwrap_or("command"),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| CmdError::click("Skarbiec returned a malformed local JSON report"))
}

pub(crate) fn unknown() -> String {
    "-".to_string()
}
