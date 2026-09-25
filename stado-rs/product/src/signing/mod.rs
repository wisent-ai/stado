mod bundle;
pub mod command;
mod constants;
mod core;
mod credentials;
mod policy;
mod signer;

use anyhow::{Context, Result};
pub use command::stage::stage;
pub use command::{acceptable, run};
pub use core::{identifier, inspect, native};
pub use policy::Policy;
use serde_json::Value;
pub use signer::Signer;
use std::path::Path;

pub fn sign(path: &Path, identifier: &str, previous: Option<&Path>) -> Result<Value> {
    let path = core::absolute(path)?;
    let mut signer = Signer::new(path.parent().context("signing target has no parent")?, None)?;
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

pub fn prepare(source: &Path, destination: &Path, product: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    if core::bundle(source) {
        sign(source, &bundle::identifier(source)?, Some(destination))?;
    } else if native(source)? {
        let previous = inspect(destination)?;
        let candidate = inspect(source)?;
        let id = if previous["state"] == "stable" {
            previous["identifier"]
                .as_str()
                .context("previous code has no identifier")?
                .to_owned()
        } else if candidate["state"] == "stable" {
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
        sign(source, &id, Some(destination))?;
    }
    Ok(())
}

pub fn verify_previous(source: &Path, destination: &Path) -> Result<()> {
    core::compatible(source, &inspect(destination)?)
}
