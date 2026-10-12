mod bundle;
pub mod command;
mod constants;
mod core;
mod credentials;
mod policy;
mod signer;

use anyhow::{Context, Result};
pub use command::stage::{stage, stage_with};
pub use command::{acceptable, run};
pub use core::{identifier, inspect, native, SignatureState};
pub use credentials::{Choice, Source, IDENTITY_VARIABLE, KEYCHAIN_VARIABLE};
pub use policy::Policy;
use serde_json::Value;
pub use signer::Signer;
use std::path::Path;

pub fn sign(path: &Path, identifier: &str, previous: Option<&Path>) -> Result<Value> {
    sign_with(&Choice::default(), path, identifier, previous)
}

/// Sign with the source and identity `choice` names (the command line's
/// signing choice), else the environment's.
pub fn sign_with(
    choice: &Choice,
    path: &Path,
    identifier: &str,
    previous: Option<&Path>,
) -> Result<Value> {
    let path = core::absolute(path)?;
    let mut signer = Signer::chosen(choice, None)?;
    let result = signer.sign(&path, identifier, previous, &Policy::default());
    let cleanup = signer.close();
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Err(cleanup)) => Err(error.context(format!(
            "signing credential cleanup also failed: {cleanup:#}"
        ))),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}

/// Sign `source` for `destination` with the identity the installed code
/// keeps, and answer the identity change (`previous` -> `new`) when the
/// operator named another identity.
pub fn prepare(
    choice: &Choice,
    source: &Path,
    destination: &Path,
    product: &str,
) -> Result<Option<Value>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let signed = if core::bundle(source) {
        sign_with(
            choice,
            source,
            &bundle::identifier(source)?,
            Some(destination),
        )?
    } else if native(source)? {
        let previous = inspect(destination)?;
        let candidate = inspect(source)?;
        let id = if crate::signing::SignatureState::is_stable(&previous) {
            previous["identifier"]
                .as_str()
                .context("previous code has no identifier")?
                .to_owned()
        } else if crate::signing::SignatureState::is_stable(&candidate) {
            candidate["identifier"]
                .as_str()
                .context("candidate has no identifier")?
                .to_owned()
        } else {
            identifier(
                product,
                destination
                    .file_name()
                    .and_then(|s| s.to_str())
                    .context("destination filename is missing")?,
            )?
        };
        sign_with(choice, source, &id, Some(destination))?
    } else {
        return Ok(None);
    };
    Ok(signed.get("identity_change").cloned())
}

pub fn verify_previous(source: &Path, destination: &Path) -> Result<()> {
    core::compatible(source, &inspect(destination)?)
}
