//! Whether an action may run now, asked of Oko's one admission decision
//! (`oko admit`). Stado keeps no copy of any ruling: a refused action is
//! refused with the ruling Oko quotes, and admitted once a later ruling of
//! the operator lifts it.

use super::steps::process::run;
use crate::cli::CmdError;

/// Oko's program; Stado reads rulings through its CLI only.
const OKO: &str = "oko";

/// The action class a ruling constrains that compiling a product is.
pub const BUILD: &str = "build";
/// The action class a ruling constrains that publishing a release is.
pub const RELEASE: &str = "release";
/// The action class a ruling constrains that installing a product is.
pub const INSTALL: &str = "install";

/// Ok when `action` may run now for `product` (every product when none is
/// named) as work serving the open defects `purposes` (`oko admit …
/// --purpose <defect>`: a consent the operator limited to a purpose covers
/// only work that names it); otherwise the refusal Oko gives, which names
/// the ruling, its words and what it waits on, prefixed with what was asked.
pub async fn refusal(action: &str, product: Option<&str>, purposes: &[&str]) -> Result<(), String> {
    let mut words = vec!["admit", action];
    if let Some(product) = product {
        words.extend(["--product", product]);
    }
    for purpose in purposes {
        words.extend(["--purpose", *purpose]);
    }
    let scope = match product {
        Some(product) => format!("{action} of {product}"),
        None => action.to_string(),
    };
    run(OKO, &words, None).await.map(|_| ()).map_err(|refusal| {
        format!("{scope} is not run: the operator's rulings do not allow it now ({refusal})")
    })
}

/// [`refusal`] for a product operation that compiles or publishes, serving
/// no named purpose: refused, nothing was exported or built.
pub async fn require(action: &str, product: Option<&str>) -> Result<(), CmdError> {
    refusal(action, product, &[])
        .await
        .map_err(|refused| CmdError::refused(format!("{refused}; nothing was exported or built")))
}
