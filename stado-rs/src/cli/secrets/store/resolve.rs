//! Where a credential operation resolves to: the admin client, the installed
//! Skarbiec, the owner vault and the keychain launcher.

use serde_json::Value;

use crate::cli::CmdError;

pub(crate) fn client() -> Result<crate::skarbiec::Client, CmdError> {
    let credentials = crate::credential_store::admin_credentials().map_err(CmdError::from)?;
    crate::skarbiec::Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(CmdError::from)
}

/// The installed Skarbiec, resolved where every owner-path write resolves it.
pub(crate) fn skarbiec_binary() -> Result<std::path::PathBuf, CmdError> {
    crate::credential_store::owner::binary().map_err(CmdError::from)
}

const SKARBIEC_LAUNCHER_CANDIDATES: &[&str] = &["$HOME/.stado/bin/skarbiec-keychain-launcher"];

pub(crate) fn skarbiec_launcher() -> Result<std::path::PathBuf, CmdError> {
    let home = crate::config_file::home().map_err(CmdError::declaration)?;
    // A launcher named in a value that is not text is refused, not passed
    // over for the installed one.
    match std::env::var("SKARBIEC_LAUNCHER") {
        Ok(explicit) => {
            let path = std::path::PathBuf::from(&explicit);
            if !path.is_file() {
                return Err(CmdError::declaration(format!(
                    "SKARBIEC_LAUNCHER names no file: {explicit}"
                )));
            }
            return Ok(path);
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => {
            return Err(CmdError::declaration(format!(
                "SKARBIEC_LAUNCHER cannot be read: {error}"
            )))
        }
    }
    for candidate in SKARBIEC_LAUNCHER_CANDIDATES {
        let path = match candidate.strip_prefix("$HOME/") {
            Some(rest) => home.join(rest),
            None => std::path::PathBuf::from(candidate),
        };
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(CmdError::click(format!(
        "no installed Skarbiec launcher at {}",
        SKARBIEC_LAUNCHER_CANDIDATES.join(", ")
    ))
    .stating(crate::primitives::failure::FailureCode::NotFound))
}

pub(crate) fn launcher_json(
    binary: &std::path::Path,
    vault: &std::path::Path,
    arguments: &[&str],
) -> Result<Value, CmdError> {
    let output = crate::wait::output(
        std::process::Command::new(binary)
            .args(arguments)
            .env("SKARBIEC_VAULT_FILE", vault)
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    )?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} {} ended with {}: {}",
            binary.display(),
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    // The parser's position is said, not the answer: `get <item>` answers an
    // item's secret fields, which no refusal may carry into a log.
    serde_json::from_slice(&output.stdout).map_err(|error| {
        CmdError::click(format!(
            "{} {} answered a local report that is not JSON: {error}",
            binary.display(),
            arguments.join(" ")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}

pub(crate) fn unknown() -> String {
    "-".to_string()
}
