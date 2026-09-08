//! Reading the shipped declaration, and resolving operator words against it.

use std::sync::LazyLock;

use crate::deploy::DeployError;

use super::declaration::{Declaration, Install, Product, Readback};
use super::validation::validate;
use super::{DECLARATION, DECLARATION_PATH, PLATFORMS};

// ---------------------------------------------------------------------------
// Reading the declaration
// ---------------------------------------------------------------------------

/// The shipped declaration, parsed and validated exactly once.
static SHIPPED: LazyLock<Result<Vec<Product>, String>> = LazyLock::new(|| parse(DECLARATION));

/// Parse and validate one declaration document.
///
/// Public because the shipped document is not the only thing that has to be
/// refusable: the rules below are the contract, and a test proves them
/// against documents this repository must never ship.
pub fn parse(text: &str) -> Result<Vec<Product>, String> {
    let declaration: Declaration = serde_json::from_str(text)
        .map_err(|error| format!("{DECLARATION_PATH} is not a valid declaration: {error}"))?;
    validate(&declaration)?;
    Ok(declaration.products)
}

/// Every product this fleet declares, in declaration order.
pub fn declared() -> Result<&'static [Product], DeployError> {
    match &*SHIPPED {
        Ok(products) => Ok(products.as_slice()),
        Err(error) => Err(DeployError(error.clone())),
    }
}

/// The deliverable set, as an operator reads it after a refusal.
pub fn allowed() -> String {
    match &*SHIPPED {
        Ok(products) => products
            .iter()
            .map(|entry| format!("  {} — {}", entry.name, entry.why))
            .collect::<Vec<String>>()
            .join("\n"),
        Err(error) => format!("  (none: {error})"),
    }
}

/// Resolve an operator's `--binary` word against the declaration.
pub fn product(name: &str) -> Result<&'static Product, DeployError> {
    declared()?
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            DeployError(format!(
                "{name:?} is not a stado-managed binary. Deliverable binaries:\n{}",
                allowed()
            ))
        })
}

/// Every declared product that installs a single program under one root, as
/// `(name, root, version argument, version shape)`.
///
/// The one caller is [`crate::deploy::host_inventory`], which reads
/// `$HOME/.stado/bin` on a host and used to loop over the two names spelled
/// into its remote program. A tree product is absent on purpose: nothing
/// under that directory belongs to one.
pub fn installed_programs(
) -> Result<Vec<(&'static str, &'static str, &'static str, &'static str)>, DeployError> {
    Ok(declared()?
        .iter()
        .filter_map(|entry| match (&entry.install, &entry.readback) {
            (Install::Program { root }, Readback::Program { argument, shape }) => Some((
                entry.name.as_str(),
                root.as_str(),
                argument.as_str(),
                shape.as_str(),
            )),
            _ => None,
        })
        .collect())
}

/// Resolve a platform word against [`PLATFORMS`].
pub fn managed_platform(platform: &str) -> Result<&'static str, DeployError> {
    PLATFORMS
        .iter()
        .find(|candidate| **candidate == platform)
        .copied()
        .ok_or_else(|| {
            DeployError(format!(
                "{platform:?} is not a published release platform; expected one of {}",
                PLATFORMS.join(", ")
            ))
        })
}
