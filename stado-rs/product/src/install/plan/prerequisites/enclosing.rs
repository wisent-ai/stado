//! The installations in progress that the one being planned is a dependency
//! of, as a parent process hands them to the binary that finishes one of
//! their dependencies.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// The installations in progress a parent process hands to the binary that
/// finishes one of their dependencies (`stado product finish`), as JSON.
pub const ENCLOSING_ENV: &str = "STADO_PRODUCT_ENCLOSING";

/// An installation in progress that the one being planned is a dependency
/// of: the product and surface it installs and the revision it builds.
#[derive(Clone, Deserialize, Serialize)]
pub struct Enclosing {
    pub product: String,
    pub surface: String,
    pub revision: String,
}

impl Enclosing {
    pub fn new(product: &str, surface: &str, revision: &str) -> Self {
        Self {
            product: product.to_owned(),
            surface: surface.to_owned(),
            revision: revision.to_owned(),
        }
    }

    pub fn node(&self) -> String {
        format!("{}/{}", self.product, self.surface)
    }
}

/// The enclosing installations this process was handed in `ENCLOSING_ENV`;
/// none when it was handed none, and a value that is not that list is
/// refused naming the variable.
pub(super) fn handed() -> Result<Vec<Enclosing>> {
    match std::env::var(ENCLOSING_ENV) {
        Err(std::env::VarError::NotPresent) => Ok(Vec::new()),
        Err(error) => bail!("{ENCLOSING_ENV} cannot be read: {error}"),
        Ok(text) => serde_json::from_str(&text).with_context(|| {
            format!("{ENCLOSING_ENV} holds no list of enclosing installations: {text}")
        }),
    }
}
