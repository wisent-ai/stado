use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Product processes no unit owns
// ---------------------------------------------------------------------------

/// Where `service deploy --from-artifact` installs the programs it renders
/// units around. Not a product root — the product declaration cannot name it,
/// because what lands there is whatever an operator deployed — and a program
/// running out of it is this fleet's program all the same.
pub const DEPLOYED_SERVICES_ROOT: &str = "$HOME/.stado/services";

/// What [`product_guess`] says about a command line that matches a managed
/// root and no product in the declaration.
pub const UNKNOWN_PRODUCT: &str = "unknown";

/// The `$HOME`-relative roots a managed program can execute out of: every
/// declared product's install root, plus [`DEPLOYED_SERVICES_ROOT`].
///
/// This is the whole definition of "a product process" for
/// [`unowned_processes`]. It comes off the shipped product declaration rather
/// than a list in this file, so a product added there is scanned for without a
/// matching edit here.
pub fn managed_roots() -> Result<Vec<String>, DeployError> {
    let mut roots = vec![DEPLOYED_SERVICES_ROOT.to_string()];
    for product in crate::deploy::products::declared()? {
        let root = product.root().to_string();
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// Which product a command line belongs to.
///
/// The host reports absolute paths and the declaration is `$HOME`-relative, so
/// both are matched on the tail they share. A program product is identified by
/// its own file name and not by its root: `stado` and `skarbiec` install into
/// the same `$HOME/.stado/bin`, and reporting a four-day-old unowned agent as
/// possibly-skarbiec would be worse than saying nothing.
pub fn product_guess(command: &str) -> String {
    let tail = |root: &str| root.strip_prefix(HOME_PREFIX).unwrap_or(root).to_string();
    let deployed = format!("{}/", tail(DEPLOYED_SERVICES_ROOT));
    if let Some((_, rest)) = command.split_once(deployed.as_str()) {
        let name = rest.split(['/', ' ']).next().unwrap_or_default();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    let products = crate::deploy::products::declared().unwrap_or_default();
    for product in products {
        if command.contains(&format!("{}/{}", tail(product.root()), product.name)) {
            return product.name.clone();
        }
    }
    for product in products {
        if command.contains(&format!("{}/", tail(product.root()))) {
            return product.name.clone();
        }
    }
    UNKNOWN_PRODUCT.to_string()
}
