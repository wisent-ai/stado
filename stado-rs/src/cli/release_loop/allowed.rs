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
//!
//! Oko answers with its verdict as JSON (`--json`): `allowed` and the
//! `reason` quoting what decided it. Only a verdict that says not allowed is
//! the operator's rulings refusing; an `oko` that gives no verdict at all (it
//! cannot be started, or it is an Oko without the `admit` verb) is said as
//! that, with its own words, since a ruling that was never read decided
//! nothing. A machine with no Oko installed at any of its declared paths has
//! no register of rulings at all: Stado runs without Oko like every product
//! runs without the others, so the action is admitted, saying that no Oko is
//! installed and so no ruling governs it.

use serde_json::Value;
use tokio::process::Command;

use crate::cli::CmdError;

use crate::deploy::host_exec::{installed_program, OKO_CLI};

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
        Err(std::env::VarError::NotPresent | std::env::VarError::NotUnicode(_)) => false,
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
    words.push("--json");
    let scope = match product {
        Some(product) => format!("{action} of {product}"),
        None => action.to_string(),
    };
    let answer = match decision(&words).await {
        Decision::Allowed(reason) => reason,
        Decision::Absent(cause) => {
            format!("no Oko is installed on this machine ({cause}), so no ruling governs {scope}")
        }
        Decision::Refused(reason) => {
            return Err(format!(
                "{scope} is not run: the operator's rulings do not allow it now ({reason})"
            ))
        }
        Decision::Unread(cause) => {
            return Err(format!(
                "{scope} is not run: Oko gave no admission decision, so whether the operator's \
                 rulings allow it was never read ({cause})"
            ))
        }
    };
    // Set before the operation starts anything, so every process it runs
    // inherits it (module doc).
    std::env::set_var(variable, format!("{} {}", std::process::id(), answer));
    Ok(())
}

/// What `oko admit … --json` answered.
enum Decision {
    /// Allowed, with the reason Oko gives.
    Allowed(String),
    /// No Oko is installed at any of its declared paths, with where it was
    /// looked for.
    Absent(String),
    /// Not allowed, with the reason quoting the ruling.
    Refused(String),
    /// No verdict: the program could not be run, or it answered something
    /// that is not one, with what it said.
    Unread(String),
}

async fn decision(words: &[&str]) -> Decision {
    // A home that is not known is not a machine without Oko: where Oko would
    // be installed was never looked at.
    let oko = match (installed_program(OKO_CLI), std::env::var_os("HOME")) {
        (Ok(oko), _) => oko,
        (Err(missing), Some(_)) => return Decision::Absent(missing),
        (Err(missing), None) => {
            return Decision::Unread(format!("oko {} did not run: {missing}", words.join(" ")))
        }
    };
    let shown = format!("{} {}", oko.display(), words.join(" "));
    let mut command = Command::new(&oko);
    command
        .args(words)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = match crate::wait::until(
        crate::wait::Kind::Process,
        &shown,
        "the caller's directory",
        command.output(),
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return Decision::Unread(format!("{shown} did not run: {error}")),
    };
    let unread = |why: String| {
        Decision::Unread(format!(
            "{shown} exited {} without a verdict ({why}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    };
    let verdict: Value = match serde_json::from_slice(&output.stdout) {
        Ok(verdict) => verdict,
        Err(error) => return unread(format!("its output is not JSON: {error}")),
    };
    match (verdict["allowed"].as_bool(), verdict["reason"].as_str()) {
        (Some(true), Some(reason)) => Decision::Allowed(reason.to_owned()),
        (Some(false), Some(reason)) => Decision::Refused(reason.to_owned()),
        _ => unread("its JSON states no boolean allowed and string reason".to_string()),
    }
}

/// [`refusal`] for a product operation that compiles or publishes, serving
/// no named purpose: refused, nothing was exported or built.
pub async fn require(action: &str, product: Option<&str>) -> Result<(), CmdError> {
    refusal(action, product, &[])
        .await
        .map_err(|refused| CmdError::refused(format!("{refused}; nothing was exported or built")))
}
