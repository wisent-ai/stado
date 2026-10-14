//! Whether an action may run now, asked of Oko's one admission decision
//! (`oko admit`). Stado keeps no copy of any ruling: a refused action is
//! refused with the ruling Oko quotes, and admitted once a later ruling of
//! the operator lifts it.
//!
//! An admitted operation is one operation with everything it runs: the
//! quality gates of a build, the real journeys those gates start and the
//! dependency installs it makes. Oko's answer is kept in this process's
//! environment under [`inherited`]'s name for the action, so each process it
//! starts inherits it and is admitted for that action without asking again —
//! a gate of an admitted build does not hang on which Oko the machine has
//! installed, and an operation is not admitted twice by two answers.

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

/// The environment variable that carries an admitted operation's admission
/// for `action` to the processes it runs: the admitted process's pid, a
/// space, and Oko's answer.
pub fn inherited(action: &str) -> String {
    format!("STADO_ADMITTED_{}", action.to_uppercase())
}

/// Whether a process that started this one was admitted for the action
/// `variable` carries. This process's own earlier admission is not one: each
/// of its operations (a delivery plan's every step) is asked. A value Stado
/// did not write (not Unicode, no pid) admits nothing, so Oko is asked.
fn admitted_above(variable: &str) -> bool {
    match std::env::var(variable) {
        Ok(value) => match value.split_once(' ') {
            Some((pid, _answer)) => pid != std::process::id().to_string(),
            None => false,
        },
        Err(_) => false,
    }
}

/// Ok when `action` may run now for `product` (every product when none is
/// named) as work serving the open defects `purposes` (`oko admit …
/// --purpose <defect>`: a consent the operator limited to a purpose covers
/// only work that names it); otherwise the refusal Oko gives, which names
/// the ruling, its words and what it waits on, prefixed with what was asked.
/// A process an admitted operation started is admitted by that operation for
/// the same action (module doc).
pub async fn refusal(action: &str, product: Option<&str>, purposes: &[&str]) -> Result<(), String> {
    let variable = inherited(action);
    if admitted_above(&variable) {
        return Ok(());
    }
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
    let answer = run(OKO, &words, None).await.map_err(|refusal| {
        format!("{scope} is not run: the operator's rulings do not allow it now ({refusal})")
    })?;
    // Set before the operation starts anything, so every process it runs
    // inherits it (module doc).
    std::env::set_var(
        variable,
        format!("{} {}", std::process::id(), answer.trim()),
    );
    Ok(())
}

/// [`refusal`] for a product operation that compiles or publishes, serving
/// no named purpose: refused, nothing was exported or built.
pub async fn require(action: &str, product: Option<&str>) -> Result<(), CmdError> {
    refusal(action, product, &[])
        .await
        .map_err(|refused| CmdError::refused(format!("{refused}; nothing was exported or built")))
}
