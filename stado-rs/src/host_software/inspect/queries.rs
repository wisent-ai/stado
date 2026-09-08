//! The only version commands a reading may run, and where they may run.

use std::collections::BTreeMap;

use crate::deploy::products::{self, Install, Readback, Shape};
use crate::deploy::DeployError;

/// One version command the shipped product declaration explicitly supports.
#[derive(Debug, Clone)]
pub(super) struct VersionQuery {
    pub(super) argument: String,
    pub(super) shape: Shape,
}

/// Exact installed paths are the safety boundary for executable probes.
///
/// A basename is not enough: `$HOME/.stado/bin/stado.previous` and an
/// unrelated service binary called `stado` did not come from the catalog's
/// install declaration and must not be executed just because their names
/// resemble one that did.
pub(super) fn version_queries(home: &str) -> Result<BTreeMap<String, VersionQuery>, DeployError> {
    let mut queries = BTreeMap::new();
    for product in products::declared()? {
        let (Install::Program { root }, Readback::Program { argument, shape }) =
            (&product.install, &product.readback)
        else {
            continue;
        };
        let root = root
            .strip_prefix("$HOME/")
            .map_or_else(|| root.to_string(), |relative| format!("{home}/{relative}"));
        queries.insert(
            format!("{root}/{}", product.name),
            VersionQuery {
                argument: argument.clone(),
                shape: *shape,
            },
        );
    }
    Ok(queries)
}
