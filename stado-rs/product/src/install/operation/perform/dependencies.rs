//! The catalogue dependencies of one product installation. Every dependency
//! is soft: every product runs without the others, so an installation never
//! installs, rebuilds or repairs another product for itself. A dependency
//! installed on this machine is used as it is installed; one an installation
//! in progress above this one places is met by it; one that is not installed,
//! or that `--without` names, is gone without, and the installation says what
//! its product does without it (the catalogue's `alternative`, or what binds
//! it by design, `bound_because`).

use super::super::super::plan::prerequisites::Enclosing;
use crate::catalog;
use crate::common::Runtime;
use anyhow::{Context, Result};
use serde_json::{json, Value};

/// The installation whose dependencies are read: what `perform` was asked
/// for, with the choices it settled.
pub(super) struct Request<'a> {
    pub runtime: &'a Runtime,
    pub product: &'a Value,
    pub id: &'a str,
    pub surface: &'a str,
    pub without: &'a [String],
}

/// What the product goes without when `dependency` is not there: its
/// alternative, what binds it by design, or that the catalogue says neither.
fn without_it(dependency: &Value) -> String {
    match (dependency["alternative"].as_str(), dependency["bound_because"].as_str()) {
        (Some(alternative), _) => alternative.to_owned(),
        (None, Some(because)) => format!("the catalogue binds it by design, so what needs it does not work: {because}"),
        (None, None) => "the catalogue declares no alternative, so what needs it does not work".to_owned(),
    }
}

/// What became of every dependency the catalogue declares for `request`'s
/// surface, in the receipt's `dependencies` form. Nothing is installed.
pub(super) fn install(request: Request<'_>, stack: &mut Vec<Enclosing>) -> Result<Vec<Value>> {
    let Request { runtime, product, id, surface, without } = request;
    let mut dependencies = Vec::new();
    let Some(declared) = product.get("dependencies") else {
        return Ok(dependencies);
    };
    for dependency in declared.as_array().context("product dependencies must be an array")? {
        if dependency["for_surface"].as_str().is_some_and(|s| s != surface) {
            continue;
        }
        let dependency_id = catalog::text(dependency, "product")?;
        let dependency_surface = catalog::text(dependency, "surface")?;
        let dependency_node = format!("{dependency_id}/{dependency_surface}");
        if without.iter().any(|name| name == dependency_id) {
            let gone = without_it(dependency);
            eprintln!("{id}: installed without {dependency_node}: {gone}");
            dependencies.push(json!({"product": dependency_id, "surface": dependency_surface,
                "checked": false, "without": true, "alternative": gone}));
            continue;
        }
        // Products may depend on each other. A dependency that is an
        // installation already in progress above this one (Tama depending
        // back on the Oko being installed) is placed by that installation
        // once this one is done, so it is met.
        if stack.iter().any(|frame| frame.node() == dependency_node) {
            let reason = "the installation in progress that this one is a dependency of places it";
            eprintln!("{id}: {dependency_node} is met: {reason}");
            dependencies.push(json!({"product": dependency_id, "surface": dependency_surface,
                "checked": false, "enclosing": true, "reason": reason}));
            continue;
        }
        let installed = crate::state::ProductState::load(runtime, dependency_id, dependency_surface)?
            .is_some_and(|state| state.status.complete());
        if installed {
            eprintln!(
                "{id}: uses {dependency_node} as it is installed; an installation installs, \
                 rebuilds or repairs no other product"
            );
            dependencies.push(json!({"product": dependency_id, "surface": dependency_surface,
                "checked": false, "used_as_installed": true}));
        } else {
            let gone = without_it(dependency);
            eprintln!("{id}: installed without {dependency_node}, which is not installed here: {gone}");
            dependencies.push(json!({"product": dependency_id, "surface": dependency_surface,
                "checked": false, "without": true, "alternative": gone}));
        }
    }
    Ok(dependencies)
}
